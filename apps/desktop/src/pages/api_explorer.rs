//! API Explorer (as the web app's API Explorer page): Gateway's operations from `GET /v1/openapi.json`,
//! grouped by its tags and filtered by path or summary. Pick one to read its parameters and schemas, fill them, edit a
//! JSON body that starts from the schema's example, and send it with the person's sign-in. The answer shows as
//! indented JSON. A list, then the operation, with Back, inside this page.
//!
//! Unlike the web page, a call goes through the app's Gateway client, so the answer has no HTTP status or timing: a
//! refusal shows Gateway's message. A DELETE asks for a second press before it is sent.

use std::time::Instant;

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Map, Value};

use super::{Ctx, Load, Verb, fetch, palette, ui};

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// The gateway's own tag in openapi.json, listed first.
const CORE: &str = "Nexus Gateway";
/// How deep `$ref`s are inlined, as the web page.
const REF_DEPTH: usize = 4;

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter endpoints"));
        let body = cx.new(|cx| TextareaState::new(window, cx).rows(10));
        let subscription = cx.observe(&filter, |_, _, cx| cx.notify());
        let mut page = ApiExplorerPage {
            ctx,
            spec: Load::Loading,
            filter,
            body,
            selected: None,
            params: Vec::new(),
            sending: false,
            generation: 0,
            confirm_delete: false,
            result: None,
            copied: false,
            _subscription: subscription,
        };
        page.load(cx);
        page
    });
    Some(view.into())
}

/// One operation of the spec.
#[derive(Debug, Clone, PartialEq)]
struct Endpoint {
    /// `get:/v1/spend`, as the web page's anchors.
    id: String,
    /// Lower case, as in the spec.
    method: String,
    path: String,
    summary: String,
    tag: String,
    /// `security: []`: no key needed.
    public: bool,
    path_params: Vec<Param>,
    query_params: Vec<Param>,
    /// The JSON body's schema, `$ref`s inlined.
    request: Option<Value>,
    /// The first 2xx answer with a JSON schema: its status and schema.
    response: Option<(String, Value)>,
}

#[derive(Debug, Clone, PartialEq)]
struct Param {
    name: String,
    required: bool,
    kind: String,
    description: String,
}

/// The spec's tags in its order, each with its operations.
#[derive(Debug, Clone, PartialEq)]
struct Spec {
    title: String,
    groups: Vec<(String, Vec<Endpoint>)>,
}

impl Spec {
    fn all(&self) -> impl Iterator<Item = &Endpoint> {
        self.groups.iter().flat_map(|(_, endpoints)| endpoints.iter())
    }

    fn find(&self, id: &str) -> Option<&Endpoint> {
        self.all().find(|endpoint| endpoint.id == id)
    }
}

/// A field of a schema, for the fields table.
#[derive(Debug, Clone, PartialEq)]
struct Field {
    name: String,
    depth: usize,
    kind: String,
    required: bool,
    description: String,
}

const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

fn read_spec(spec: &Value) -> Spec {
    let mut groups: Vec<(String, Vec<Endpoint>)> = spec
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| tag.get("name").and_then(Value::as_str))
                .map(|name| (name.to_string(), Vec::new()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(paths) = spec.get("paths").and_then(Value::as_object) {
        for (path, methods) in paths {
            let Some(methods) = methods.as_object() else { continue };
            for (method, op) in methods {
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                let endpoint = read_endpoint(spec, path, method, op);
                match groups.iter_mut().find(|(tag, _)| *tag == endpoint.tag) {
                    Some((_, endpoints)) => endpoints.push(endpoint),
                    None => groups.push((endpoint.tag.clone(), vec![endpoint])),
                }
            }
        }
    }
    groups.retain(|(_, endpoints)| !endpoints.is_empty());
    // Gateway's own first, then connectors, then each product.
    groups.sort_by_key(|(tag, _)| !(tag == CORE || tag.starts_with(&format!("{CORE} ›")) || tag == "Connectors"));
    Spec {
        title: spec
            .pointer("/info/title")
            .and_then(Value::as_str)
            .unwrap_or("API")
            .to_string(),
        groups,
    }
}

fn read_endpoint(spec: &Value, path: &str, method: &str, op: &Value) -> Endpoint {
    let params: Vec<Value> = op
        .get("parameters")
        .and_then(Value::as_array)
        .map(|params| params.iter().map(|param| resolve(param, spec, 0)).collect())
        .unwrap_or_default();
    let read_params = |place: &str| -> Vec<Param> {
        params
            .iter()
            .filter(|param| param.get("in").and_then(Value::as_str) == Some(place))
            .filter_map(|param| {
                Some(Param {
                    name: param.get("name")?.as_str()?.to_string(),
                    required: param.get("required").and_then(Value::as_bool).unwrap_or(false),
                    kind: param.get("schema").map(type_of).unwrap_or_else(|| "string".into()),
                    description: ui::text(param, "description").to_string(),
                })
            })
            .collect()
    };
    let json_schema = |value: &Value| value.pointer("/content/application~1json/schema").cloned();
    let request = op
        .get("requestBody")
        .map(|body| resolve(body, spec, 0))
        .and_then(|body| json_schema(&body))
        .map(|schema| resolve(&schema, spec, 0));
    let response = op.get("responses").and_then(Value::as_object).and_then(|responses| {
        responses.iter().find_map(|(status, answer)| {
            if !status.starts_with('2') {
                return None;
            }
            let answer = resolve(answer, spec, 0);
            json_schema(&answer).map(|schema| (status.clone(), resolve(&schema, spec, 0)))
        })
    });
    Endpoint {
        id: format!("{method}:{path}"),
        method: method.to_string(),
        path: path.to_string(),
        summary: ui::text(op, "summary").to_string(),
        tag: op
            .pointer("/tags/0")
            .and_then(Value::as_str)
            .unwrap_or("Other")
            .to_string(),
        public: op
            .get("security")
            .and_then(Value::as_array)
            .is_some_and(|security| security.is_empty()),
        path_params: read_params("path"),
        query_params: read_params("query"),
        request,
        response,
    }
}

/// `$ref`s inlined, to a small depth, so a schema reads on its own.
fn resolve(value: &Value, spec: &Value, depth: usize) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|item| resolve(item, spec, depth)).collect()),
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
                if depth > REF_DEPTH {
                    return value.clone();
                }
                // `#/components/schemas/Name` as a JSON pointer.
                let target = reference
                    .strip_prefix('#')
                    .and_then(|pointer| spec.pointer(pointer))
                    .cloned()
                    .unwrap_or(Value::Object(Map::new()));
                return resolve(&target, spec, depth + 1);
            }
            Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), resolve(value, spec, depth)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

fn type_of(schema: &Value) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values.iter().map(Value::to_string).collect::<Vec<_>>().join(" | ");
    }
    match schema.get("type") {
        Some(Value::String(kind)) if kind == "array" => {
            format!("{}[]", schema.get("items").map(type_of).unwrap_or_else(|| "any".into()))
        }
        Some(Value::String(kind)) => kind.clone(),
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | "),
        _ if schema.get("oneOf").is_some() || schema.get("anyOf").is_some() => "one of".into(),
        _ if schema.get("allOf").is_some() || schema.get("properties").is_some() => "object".into(),
        _ => "any".into(),
    }
}

/// `allOf` parts merged into one object schema.
fn merged(schema: &Value) -> Value {
    let Some(parts) = schema.get("allOf").and_then(Value::as_array) else {
        return schema.clone();
    };
    let mut properties = Map::new();
    let mut required = Vec::new();
    for part in parts.iter().map(merged) {
        if let Some(own) = part.get("properties").and_then(Value::as_object) {
            properties.extend(own.clone());
        }
        if let Some(own) = part.get("required").and_then(Value::as_array) {
            required.extend(own.iter().cloned());
        }
    }
    serde_json::json!({ "type": "object", "properties": properties, "required": required })
}

/// A schema as rows of fields: name (by depth), type, required, description.
fn flatten(schema: &Value, depth: usize, out: &mut Vec<Field>) {
    let target = if schema.get("type").and_then(Value::as_str) == Some("array") {
        merged(schema.get("items").unwrap_or(&Value::Null))
    } else {
        merged(schema)
    };
    let Some(properties) = target.get("properties").and_then(Value::as_object) else {
        return;
    };
    let required: Vec<&str> = target
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (name, property) in properties {
        out.push(Field {
            name: name.clone(),
            depth,
            kind: type_of(property),
            required: required.contains(&name.as_str()),
            description: property
                .get("description")
                .or_else(|| property.get("format"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
        if depth < 3 && out.len() < 200 {
            flatten(property, depth + 1, out);
        }
    }
}

/// A starting body from a schema: its example, or an empty value of each required field's type.
fn example(schema: &Value) -> Value {
    if let Some(first) = schema.get("examples").and_then(Value::as_array).and_then(|e| e.first()) {
        return first.clone();
    }
    let schema = merged(schema);
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let properties = schema.get("properties").and_then(Value::as_object);
            let required: Option<Vec<&str>> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).collect());
            let mut out = Map::new();
            for (name, value) in properties.into_iter().flatten() {
                if required
                    .as_ref()
                    .is_none_or(|required| required.contains(&name.as_str()))
                {
                    out.insert(name.clone(), example(value));
                }
            }
            Value::Object(out)
        }
        Some("array") => Value::Array(schema.get("items").map(example).into_iter().collect()),
        Some("integer" | "number") => Value::from(0),
        Some("boolean") => Value::Bool(false),
        _ => schema
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|values| values.first().cloned())
            .unwrap_or(Value::String(String::new())),
    }
}

fn verb(method: &str) -> Option<Verb> {
    Some(match method {
        "get" => Verb::Get,
        "post" => Verb::Post,
        "put" => Verb::Put,
        "patch" => Verb::Patch,
        "delete" => Verb::Delete,
        _ => return None,
    })
}

/// Percent-encoding for a path segment or a query value.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The path with its parameters filled, and the query string. A parameter left empty stays as `{name}`.
fn request_url(path: &str, values: &[(String, bool, String)]) -> String {
    let mut url = path.to_string();
    let mut query = Vec::new();
    for (name, in_query, value) in values {
        if value.is_empty() {
            continue;
        }
        if *in_query {
            query.push(format!("{}={}", encode(name), encode(value)));
        } else {
            url = url.replace(&format!("{{{name}}}"), &encode(value));
        }
    }
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    url
}

/// Refuse incomplete calls locally, including required query parameters with the same name as a path parameter.
fn validate_parameters(endpoint: &Endpoint, values: &[(String, bool, String)]) -> Result<(), String> {
    for (in_query, params) in [(false, &endpoint.path_params), (true, &endpoint.query_params)] {
        for param in params.iter().filter(|param| param.required || !in_query) {
            if !values
                .iter()
                .any(|(name, query, value)| name == &param.name && *query == in_query && !value.trim().is_empty())
            {
                return Err(format!(
                    "Enter the required {} parameter: {}.",
                    if in_query { "query" } else { "path" },
                    param.name
                ));
            }
        }
    }
    Ok(())
}

/// What a send gave back.
#[derive(Debug, Clone)]
struct Answer {
    ok: bool,
    text: String,
    ms: u128,
}

struct ApiExplorerPage {
    ctx: Ctx,
    spec: Load<Spec>,
    filter: Entity<InputState>,
    body: Entity<TextareaState>,
    /// The operation open, by id; None: the list.
    selected: Option<String>,
    /// Its path and query parameters: name, in the query, input.
    params: Vec<(String, bool, Entity<InputState>)>,
    sending: bool,
    /// Invalidate responses when a different operation is opened.
    generation: u64,
    /// A DELETE's Send was pressed once.
    confirm_delete: bool,
    result: Option<Answer>,
    copied: bool,
    _subscription: Subscription,
}

impl ApiExplorerPage {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.spec = Load::Loading;
        let request = self.ctx.gateway.query("/v1/openapi.json".into());
        super::read(cx, request, |this, answer, _| {
            this.spec = match answer {
                Ok(spec) if spec.get("paths").is_some() => Load::Ready(read_spec(&spec)),
                Ok(_) => Load::Failed("Gateway's answer is not an API description.".into()),
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    fn open(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(spec) = &self.spec else {
            return;
        };
        let Some(endpoint) = spec.find(&id).cloned() else {
            return;
        };
        self.params = endpoint
            .path_params
            .iter()
            .map(|param| (param, false))
            .chain(endpoint.query_params.iter().map(|param| (param, true)))
            .map(|(param, in_query)| {
                let hint = if in_query && !param.required {
                    "optional".to_string()
                } else {
                    param.name.clone()
                };
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
                cx.observe(&input, |_, _, cx| cx.notify()).detach();
                (param.name.clone(), in_query, input)
            })
            .collect();
        let body = endpoint
            .request
            .as_ref()
            .map(|schema| serde_json::to_string_pretty(&example(schema)).unwrap_or_default())
            .unwrap_or_default();
        self.body.update(cx, |state, cx| state.set_value(body, window, cx));
        // The endpoint opens ready for its first parameter.
        if let Some((_, _, first)) = self.params.first() {
            first.update(cx, |state, cx| state.focus(window, cx));
        }
        self.generation += 1;
        self.selected = Some(id);
        self.result = None;
        self.sending = false;
        self.confirm_delete = false;
        self.copied = false;
        cx.notify();
    }

    fn values(&self, cx: &App) -> Vec<(String, bool, String)> {
        self.params
            .iter()
            .map(|(name, in_query, input)| (name.clone(), *in_query, input.read(cx).value().trim().to_string()))
            .collect()
    }

    fn send(&mut self, endpoint: &Endpoint, cx: &mut Context<Self>) {
        let Some(verb) = verb(&endpoint.method) else {
            return;
        };
        if self.sending {
            return;
        }
        let values = self.values(cx);
        if let Err(error) = validate_parameters(endpoint, &values) {
            self.result = Some(Answer {
                ok: false,
                text: error,
                ms: 0,
            });
            self.confirm_delete = false;
            cx.notify();
            return;
        }
        if endpoint.method == "delete" && !self.confirm_delete {
            self.confirm_delete = true;
            cx.notify();
            return;
        }
        self.confirm_delete = false;
        let body = if endpoint.request.is_some() {
            let text = self.body.read(cx).value().to_string();
            match serde_json::from_str::<Value>(&text) {
                Ok(body) => Some(body),
                Err(error) => {
                    self.result = Some(Answer {
                        ok: false,
                        text: format!("The body is not JSON: {error}"),
                        ms: 0,
                    });
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let url = request_url(&endpoint.path, &values);
        self.sending = true;
        self.result = None;
        let started = Instant::now();
        let generation = self.generation;
        let request = self.ctx.gateway.request(verb, url, body);
        fetch(cx, request, move |this, answer, _| {
            if this.generation != generation {
                return;
            }
            this.sending = false;
            let ms = started.elapsed().as_millis();
            this.result = Some(match answer {
                Ok(value) => Answer {
                    ok: true,
                    text: serde_json::to_string_pretty(&value).unwrap_or_default(),
                    ms,
                },
                Err(error) => Answer {
                    ok: false,
                    text: error,
                    ms,
                },
            });
        });
        cx.notify();
    }

    fn render_list(&self, spec: &Spec, cx: &mut Context<Self>) -> Div {
        let palette = palette(cx);
        let needle = self.filter.read(cx).value().trim().to_lowercase();
        let matches = |endpoint: &Endpoint| {
            needle.is_empty()
                || endpoint.path.to_lowercase().contains(&needle)
                || endpoint.summary.to_lowercase().contains(&needle)
        };
        let mut list = v_flex();
        let mut shown = 0;
        for (tag, endpoints) in &spec.groups {
            let endpoints: Vec<&Endpoint> = endpoints.iter().filter(|endpoint| matches(endpoint)).collect();
            if endpoints.is_empty() {
                continue;
            }
            list = list.child(
                ui::label(tag.replace("Nexus Gateway › ", ""), palette)
                    .pt(px(14.))
                    .pb(px(4.)),
            );
            for endpoint in endpoints {
                shown += 1;
                let id = endpoint.id.clone();
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("api-op-{}", endpoint.id)))
                        .min_h(px(34.))
                        .gap(px(10.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .child(method_tag(&endpoint.method, palette))
                        .child(
                            div()
                                .flex_none()
                                .font_family(ui::MONO)
                                .text_size(px(12.))
                                .text_color(palette.strong)
                                .child(endpoint.path.trim_start_matches("/v1").to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .child(endpoint.summary.clone()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.open(id.clone(), window, cx))),
                );
            }
        }
        let filter = h_flex()
            .h(px(32.))
            .px(px(10.))
            .rounded(px(8.))
            .bg(palette.hover)
            .text_size(px(13.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(Input::new(&self.filter).appearance(false)),
            );
        v_flex()
            .child(filter)
            .when(shown == 0, |column| {
                column.child(ui::note(format!("No endpoint matches “{}”.", needle), palette).py(px(16.)))
            })
            .child(list)
    }

    fn render_endpoint(&self, endpoint: &Endpoint, cx: &mut Context<Self>) -> Div {
        let palette = palette(cx);
        let values = self.values(cx);
        let url = request_url(&endpoint.path, &values);
        let missing = endpoint.path_params.iter().any(|param| {
            values
                .iter()
                .any(|(name, q, value)| !q && *name == param.name && value.is_empty())
        });
        let curl = {
            let mut lines = vec![format!(
                "curl -X {} {}{url}",
                endpoint.method.to_uppercase(),
                self.ctx.gateway.url().trim_end_matches('/')
            )];
            if !endpoint.public {
                lines.push("  -H \"Authorization: Bearer $NEXUS_API_KEY\"".into());
            }
            if endpoint.request.is_some() {
                let body = self.body.read(cx).value().to_string();
                let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
                lines.push(format!("  -H \"content-type: application/json\" -d '{body}'"));
            }
            lines.join(" \\\n")
        };

        let mut column = v_flex()
            .gap(px(10.))
            .child(
                h_flex().child(ui::button("api-back", "← All endpoints", palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.generation += 1;
                        this.sending = false;
                        this.selected = None;
                        this.result = None;
                        cx.notify();
                    },
                ))),
            )
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(method_tag(&endpoint.method, palette))
                    .child(
                        div()
                            .font_family(ui::MONO)
                            .text_size(px(15.))
                            .text_color(palette.strong)
                            .child(endpoint.path.clone()),
                    ),
            )
            .when(!endpoint.summary.is_empty(), |column| {
                column.child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .child(endpoint.summary.clone()),
                )
            })
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(ui::badge(endpoint.tag.clone(), palette))
                    .child(ui::badge(
                        if endpoint.public {
                            "No key needed"
                        } else {
                            "Your sign-in"
                        },
                        palette,
                    )),
            );

        if !self.params.is_empty() {
            column = column.child(ui::section_head("Parameters", palette));
            for (name, in_query, input) in &self.params {
                let param = if *in_query {
                    &endpoint.query_params
                } else {
                    &endpoint.path_params
                }
                .iter()
                .find(|param| param.name == *name);
                let label = format!(
                    "{}{name}{}",
                    if *in_query { "?" } else { "" },
                    if param.is_some_and(|param| param.required) {
                        " (required)"
                    } else {
                        ""
                    }
                );
                column = column.child(
                    h_flex()
                        .gap(px(12.))
                        .child(
                            v_flex()
                                .w(px(180.))
                                .flex_none()
                                .child(
                                    div()
                                        .font_family(ui::MONO)
                                        .text_size(px(12.))
                                        .text_color(palette.strong)
                                        .child(label),
                                )
                                .when_some(param, |column, param| {
                                    column.child(ui::label(param.kind.clone(), palette))
                                }),
                        )
                        .child(
                            h_flex()
                                .flex_1()
                                .h(px(30.))
                                .px(px(10.))
                                .rounded(px(8.))
                                .bg(palette.hover)
                                .text_size(px(13.))
                                .child(div().flex_1().min_w(px(0.)).child(Input::new(input).appearance(false))),
                        ),
                );
            }
        }

        if let Some(schema) = &endpoint.request {
            column = column
                .child(ui::section_head("Request body", palette))
                .child(fields_table(schema, palette))
                .child(ui::label("Body (JSON)", palette).mt(px(6.)))
                .child(
                    div()
                        .p(px(8.))
                        .rounded(px(8.))
                        .bg(palette.raised)
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .child(Textarea::new(&self.body).appearance(false)),
                );
        }
        if let Some((status, schema)) = &endpoint.response {
            column = column
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(ui::section_head("Response", palette))
                        .child(ui::badge(status.clone(), palette).mt(px(14.))),
                )
                .child(fields_table(schema, palette));
        }

        let sendable = verb(&endpoint.method).is_some();
        let send_label = if self.sending {
            "Sending…"
        } else if self.confirm_delete {
            "Press again to send DELETE"
        } else {
            "Send"
        };
        let ep = endpoint.clone();
        let copy = curl.clone();
        column = column
            .child(ui::section_head("Try it", palette))
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    .text_color(palette.foreground)
                    .child(curl),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child({
                        let enabled = sendable && !missing && !self.sending;
                        send_button(send_label, self.confirm_delete, enabled, palette).when(enabled, |button| {
                            button.on_click(cx.listener(move |this, _, _, cx| this.send(&ep, cx)))
                        })
                    })
                    .child(
                        ui::button(
                            "api-copy-curl",
                            if self.copied { "Copied" } else { "Copy as curl" },
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            this.copied = true;
                            cx.notify();
                        })),
                    )
                    .when(missing, |row| {
                        row.child(ui::note("Fill the path parameters first.", palette))
                    }),
            );
        if let Some(answer) = &self.result {
            let text = answer.text.clone();
            column = column.child(
                v_flex()
                    .rounded(px(8.))
                    .bg(palette.raised)
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .px(px(10.))
                            .py(px(6.))
                            .text_size(px(12.))
                            .child(
                                div()
                                    .text_color(if answer.ok { palette.strong } else { palette.destructive })
                                    .child(if answer.ok { "Answered" } else { "Refused" }),
                            )
                            .child(div().flex_1())
                            .child(div().text_color(palette.faint).child(format!("{} ms", answer.ms)))
                            .child(
                                div()
                                    .id("api-copy-answer")
                                    .text_color(palette.muted)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(palette.strong))
                                    .child("Copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .p(px(10.))
                            .font_family(ui::MONO)
                            .text_size(px(12.))
                            .text_color(if answer.ok {
                                palette.foreground
                            } else {
                                palette.destructive
                            })
                            .children(answer.text.lines().take(400).map(|line| {
                                div().whitespace_nowrap().child(if line.is_empty() {
                                    " ".to_string()
                                } else {
                                    line.to_string()
                                })
                            })),
                    ),
            );
        }
        column
    }
}

/// GET, POST and the rest, small and in capitals.
fn method_tag(method: &str, palette: crate::palette::Palette) -> Div {
    div()
        .flex_none()
        .w(px(52.))
        .font_family(ui::MONO)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if method == "delete" {
            palette.destructive
        } else {
            palette.muted
        })
        .child(method.to_uppercase())
}

fn fields_table(schema: &Value, palette: crate::palette::Palette) -> Div {
    let mut fields = Vec::new();
    flatten(schema, 0, &mut fields);
    if fields.is_empty() {
        return div()
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(palette.hover)
            .font_family(ui::MONO)
            .text_size(px(12.))
            .text_color(palette.foreground)
            .children(
                serde_json::to_string_pretty(schema)
                    .unwrap_or_default()
                    .lines()
                    .take(80)
                    .map(|line| div().whitespace_nowrap().child(line.to_string())),
            );
    }
    v_flex().children(fields.into_iter().map(|field| {
        h_flex()
            .items_start()
            .gap(px(12.))
            .py(px(6.))
            .text_size(px(12.))
            .child(
                h_flex()
                    .w(px(220.))
                    .flex_none()
                    .gap(px(6.))
                    .pl(px(field.depth as f32 * 14.))
                    .child(div().font_family(ui::MONO).text_color(palette.strong).child(field.name))
                    .when(field.required, |row| row.child(ui::label("required", palette))),
            )
            .child(
                div()
                    .w(px(120.))
                    .flex_none()
                    .truncate()
                    .font_family(ui::MONO)
                    .text_color(palette.muted)
                    .child(field.kind),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_color(palette.faint)
                    .child(field.description),
            )
    }))
}

fn send_button(
    label: impl Into<SharedString>,
    danger: bool,
    enabled: bool,
    palette: crate::palette::Palette,
) -> Stateful<Div> {
    if !enabled {
        ui::disabled_button("api-send", label, palette)
    } else if danger {
        ui::danger_button("api-send", label, palette)
    } else {
        ui::primary_button("api-send", label, palette)
    }
}

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex()
            .child(ui::section_head("Try it", palette))
            .child(send_button("Send request", false, false, palette))
            .into_any_element()
    })
}

impl Render for ApiExplorerPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let column = v_flex().child(ui::lead(
            "Pick an endpoint, read its schema, and send it with your sign-in.",
            palette,
        ));
        let spec =
            match &self.spec {
                Load::Loading => return column.child(ui::skeleton(6, palette)),
                Load::Failed(error) => {
                    return column
                        .child(ui::failed(error.clone(), palette))
                        .child(h_flex().justify_center().child(
                            ui::button("api-retry", "Try again", palette).on_click(cx.listener(|this, _, _, cx| {
                                this.load(cx);
                                cx.notify();
                            })),
                        ));
                }
                Load::Ready(spec) => spec.clone(),
            };
        if spec.groups.is_empty() {
            return column.child(ui::empty(
                "No endpoints",
                "The API description lists no paths.",
                palette,
            ));
        }
        let open = self.selected.as_deref().and_then(|id| spec.find(id)).cloned();
        match open {
            Some(endpoint) => column.child(self.render_endpoint(&endpoint, cx)),
            None => column
                .child(ui::label(format!("{} · {} endpoints", spec.title, spec.all().count()), palette).mb(px(8.)))
                .child(self.render_list(&spec, cx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, example, flatten, read_spec, request_url, type_of, validate_parameters};

    fn spec() -> Value {
        serde_json::from_str(
            r##"{
              "info": {"title": "Nexus Gateway", "version": "1"},
              "tags": [{"name": "Nexus Gateway › Agents"}, {"name": "Empty"}, {"name": "Nexus Gateway"}],
              "paths": {
                "/v1/agents/access": {
                  "get": {"tags": ["Nexus Gateway › Agents"], "summary": "Your routes",
                          "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/AgentAccess"}}}}}},
                  "put": {"tags": ["Nexus Gateway › Agents"], "summary": "Choose a route",
                          "parameters": [{"$ref": "#/components/parameters/IdempotencyKey"}],
                          "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "properties": {"claude": {"type": "string", "enum": ["gateway", "subscription"]}}}}}}}
                },
                "/v1/teams/{slug}": {
                  "delete": {"tags": ["Nexus Gateway"], "parameters": [{"name": "slug", "in": "path", "required": true, "schema": {"type": "string"}}, {"name": "force", "in": "query", "schema": {"type": "boolean"}}]},
                  "parameters": []
                },
                "/v1/health": {"get": {"security": [], "summary": "Health"}}
              },
              "components": {
                "schemas": {"AgentAccess": {"type": "object", "required": ["agents"], "properties": {"agentSubscriptions": {"type": "boolean"}, "agents": {"type": "array", "items": {"$ref": "#/components/schemas/Entry"}}}},
                            "Entry": {"type": "object", "properties": {"agent": {"type": "string", "description": "claude or codex"}}}},
                "parameters": {"IdempotencyKey": {"name": "Idempotency-Key", "in": "header"}}
              }
            }"##,
        )
        .expect("JSON")
    }

    #[test]
    fn reads_the_operations_by_tag() {
        let spec = read_spec(&spec());
        let tags: Vec<&str> = spec.groups.iter().map(|(tag, _)| tag.as_str()).collect();
        assert_eq!(
            tags,
            ["Nexus Gateway › Agents", "Nexus Gateway", "Other"],
            "empty tags go, untagged is Other"
        );
        let put = spec.find("put:/v1/agents/access").expect("put");
        assert!(
            put.path_params.is_empty() && put.query_params.is_empty(),
            "header parameters are not shown"
        );
        assert_eq!(
            example(put.request.as_ref().unwrap()),
            serde_json::json!({"claude": "gateway"})
        );
        let get = spec.find("get:/v1/agents/access").expect("get");
        let mut fields = Vec::new();
        flatten(&get.response.as_ref().unwrap().1, 0, &mut fields);
        let names: Vec<(&str, usize, bool)> = fields.iter().map(|f| (f.name.as_str(), f.depth, f.required)).collect();
        assert_eq!(
            names,
            [
                ("agentSubscriptions", 0, false),
                ("agents", 0, true),
                ("agent", 1, false)
            ]
        );
        assert_eq!(fields[1].kind, "object[]");
        let delete = spec.find("delete:/v1/teams/{slug}").expect("delete");
        assert_eq!(delete.path_params[0].name, "slug");
        assert_eq!(delete.query_params[0].kind, "boolean");
        assert!(spec.find("get:/v1/health").expect("health").public);
    }

    #[test]
    fn required_parameters_are_checked_in_their_own_location() {
        let spec = read_spec(&spec());
        let mut endpoint = spec.find("delete:/v1/teams/{slug}").unwrap().clone();
        assert!(
            validate_parameters(&endpoint, &[])
                .unwrap_err()
                .contains("path parameter: slug")
        );
        let mut values = vec![("slug".into(), false, "infra".into())];
        assert!(
            validate_parameters(&endpoint, &values).is_ok(),
            "optional query may be omitted"
        );
        endpoint.query_params[0].name = "slug".into();
        endpoint.query_params[0].required = true;
        assert!(
            validate_parameters(&endpoint, &values)
                .unwrap_err()
                .contains("query parameter: slug")
        );
        values.push(("slug".into(), true, "  ".into()));
        assert!(validate_parameters(&endpoint, &values).is_err());
        values[1].2 = "true".into();
        assert!(validate_parameters(&endpoint, &values).is_ok());
    }

    #[test]
    fn fills_the_url() {
        let values = vec![
            ("slug".to_string(), false, "my team".to_string()),
            ("force".to_string(), true, "true".to_string()),
            ("cursor".to_string(), true, String::new()),
        ];
        assert_eq!(
            request_url("/v1/teams/{slug}", &values),
            "/v1/teams/my%20team?force=true"
        );
        assert_eq!(
            type_of(&serde_json::json!({"type": ["string", "null"]})),
            "string | null"
        );
    }
}
