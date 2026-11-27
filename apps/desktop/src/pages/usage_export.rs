//! Usage › Export (`ui/.../nexus-ui/usage/frame.tsx` `ExportButton`, `observe.ts`): the tab's rows as a file Gateway
//! writes, CSV or JSON Lines, saved where the person picks (`download.rs`). Overview and Breakdown save the usage family
//! (`GET /v1/analytics/usage/export.{csv|jsonl}`), Calls the calls (`GET /v1/spend/calls/export.{csv|jsonl}`), each with
//! the tab's range and filters.
//!
//! A file holds at most 100,000 rows. A JSON Lines file's last line is its manifest: whole, or cut with the cursor of
//! the next part, which "Download part N" asks for. A CSV file's manifest is on its audit event, found by the export id
//! in a response header this app's text reads do not keep; so a CSV file with fewer lines is whole, as the web reads
//! it, and a longer one says where its manifest is, as the web does when it has no export id.

use serde_json::Value;

/// The file's format: the menu's two items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Csv,
    Jsonl,
}

impl Format {
    pub(super) const ALL: [Format; 2] = [Format::Csv, Format::Jsonl];

    pub(super) fn ext(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Jsonl => "jsonl",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Jsonl => "JSON Lines",
        }
    }
}

/// Which export a tab saves: its family names the file and the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    Usage,
    SpendCalls,
}

impl Family {
    fn route(self) -> &'static str {
        match self {
            Family::Usage => "/v1/analytics/usage/export",
            Family::SpendCalls => "/v1/spend/calls/export",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Family::Usage => "usage",
            Family::SpendCalls => "spend-calls",
        }
    }
}

/// The most rows one file holds (`openapi.json`: "At most 100,000 rows a file").
pub(super) const FILE_ROW_LIMIT: usize = 100_000;

/// One export asked for: the family, the format, the filters as query pairs, and the part (1 for the first).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Ask {
    pub family: Family,
    pub format: Format,
    pub filters: Vec<(String, String)>,
    pub part: usize,
    pub cursor: Option<String>,
}

impl Ask {
    /// The route with the filters and, for a later part, the cursor.
    pub(super) fn path(&self) -> String {
        let mut pairs: Vec<(&str, &str)> = self
            .filters
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        if let Some(cursor) = &self.cursor {
            pairs.push(("cursor", cursor));
        }
        format!(
            "{}.{}{}",
            self.family.route(),
            self.format.ext(),
            super::usage::query(&pairs)
        )
    }

    /// `nexus-spend-calls-acme-2026-10-26.csv`, with `-part2` on the next part of a cut file (`fileName`).
    pub(super) fn file_name(&self, org: &str, today: &str) -> String {
        let part = if self.part > 1 {
            format!("-part{}", self.part)
        } else {
            String::new()
        };
        format!(
            "nexus-{}-{}-{today}{part}.{}",
            self.family.slug(),
            super::download::name_part(org),
            self.format.ext()
        )
    }
}

/// A JSON Lines file's manifest, its last line (`{"manifest": …}`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Manifest {
    pub complete: bool,
    pub next_cursor: Option<String>,
    pub rows: u64,
    /// The days Gateway applied, which a next part must keep.
    pub range: Option<(String, String)>,
}

pub(super) fn read_manifest(body: &str) -> Option<Manifest> {
    let last = body.trim_end().lines().last()?;
    let line: Value = serde_json::from_str(last).ok()?;
    let manifest = line.get("manifest")?;
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(String::from);
    Some(Manifest {
        complete: manifest.get("complete").and_then(Value::as_bool).unwrap_or(false),
        next_cursor: text(manifest.get("nextCursor")).filter(|cursor| !cursor.is_empty()),
        rows: manifest.get("rows").and_then(Value::as_u64).unwrap_or(0),
        range: manifest
            .get("range")
            .and_then(|range| Some((text(range.get("from"))?, text(range.get("to"))?))),
    })
}

/// Whether a CSV file may be cut: past its header it has at least the row limit's lines (`mayBeCut`). Fewer prove it
/// whole, since a quoted cell's line break only adds lines.
pub(super) fn may_be_cut(csv: &str) -> bool {
    csv.bytes().filter(|byte| *byte == b'\n').count() > FILE_ROW_LIMIT
}

/// What a saved file holds (`SavedFile`).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Held {
    /// Every row; the count when a manifest gave it.
    Whole(Option<u64>),
    /// The file stopped at its row limit; the ask for the next part.
    Cut(u64, Ask),
    /// No manifest could be read, and the file may stop at its row limit.
    Unknown,
}

pub(super) fn held(ask: &Ask, body: &str) -> Held {
    if ask.format == Format::Csv {
        return if may_be_cut(body) {
            Held::Unknown
        } else {
            Held::Whole(None)
        };
    }
    let Some(manifest) = read_manifest(body) else {
        return Held::Unknown;
    };
    match (&manifest.next_cursor, manifest.complete) {
        (_, true) => Held::Whole(Some(manifest.rows)),
        (None, false) => Held::Unknown,
        (Some(cursor), false) => {
            // The cursor is bound to the filters Gateway applied, its default days included (`nextPart`).
            let mut filters: Vec<(String, String)> = ask
                .filters
                .iter()
                .filter(|(key, _)| !(manifest.range.is_some() && (key == "from" || key == "to")))
                .cloned()
                .collect();
            if let Some((from, to)) = &manifest.range {
                filters.push(("from".into(), from.clone()));
                filters.push(("to".into(), to.clone()));
            }
            Held::Cut(
                manifest.rows,
                Ask {
                    filters,
                    part: ask.part + 1,
                    cursor: Some(cursor.clone()),
                    ..ask.clone()
                },
            )
        }
    }
}

/// The note on a saved file (`exportNote`). A part after the first says "the next" or "the last".
pub(super) fn note(name: &str, ask: &Ask, held: &Held) -> String {
    let later = ask.part > 1;
    let count = |rows: u64| super::ui::count(rows);
    match held {
        Held::Cut(rows, _) => format!(
            "Saved {name}: the {} {} rows.",
            if later { "next" } else { "first" },
            count(*rows)
        ),
        Held::Whole(None) if later => format!("Saved {name}, with the rest of the rows."),
        Held::Whole(None) => format!("Saved {name}, with every row."),
        Held::Whole(Some(rows)) => format!(
            "Saved {name}: {} {} rows.",
            if later { "the last" } else { "all" },
            count(*rows)
        ),
        Held::Unknown => match ask.format {
            Format::Csv => format!(
                "Saved {name}. It may stop at {} rows; its manifest is in Audit.",
                count(FILE_ROW_LIMIT as u64)
            ),
            Format::Jsonl => format!(
                "Saved {name}, with no manifest. It may stop at {} rows.",
                count(FILE_ROW_LIMIT as u64)
            ),
        },
    }
}

/// Why a download did not happen, as the page says it (`exportFailure`).
pub(super) fn failure(error: &str) -> String {
    match super::download::status_of(error) {
        Some(501) | Some(404) => "This Nexus Gateway can't export these yet.".into(),
        Some(403) => "Only owners and admins download files.".into(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Ask, Family, Format, Held, failure, held, note, read_manifest};

    fn ask(format: Format) -> Ask {
        Ask {
            family: Family::SpendCalls,
            format,
            filters: vec![
                ("from".into(), "2026-10-08".into()),
                ("to".into(), "2026-11-06".into()),
                ("model".into(), "anthropic/claude-opus-5-5".into()),
            ],
            part: 1,
            cursor: None,
        }
    }

    #[test]
    fn asks_for_the_tab_s_file() {
        let first = ask(Format::Csv);
        assert_eq!(
            first.path(),
            "/v1/spend/calls/export.csv?from=2026-10-08&to=2026-11-06&model=anthropic%2Fclaude-opus-5-5"
        );
        assert_eq!(
            first.file_name("acme", "2026-11-06"),
            "nexus-spend-calls-acme-2026-11-06.csv"
        );
        let usage = Ask {
            family: Family::Usage,
            format: Format::Jsonl,
            part: 2,
            ..first
        };
        assert!(usage.path().starts_with("/v1/analytics/usage/export.jsonl?from="));
        assert_eq!(
            usage.file_name("acme", "2026-11-06"),
            "nexus-usage-acme-2026-11-06-part2.jsonl"
        );
    }

    #[test]
    fn reads_a_cut_file_and_asks_for_the_next_part() {
        let body = "{\"a\":1}\n{\"manifest\":{\"complete\":false,\"nextCursor\":\"c2\",\"rows\":100000,\
                    \"range\":{\"from\":\"2026-10-01\",\"to\":\"2026-10-31\"}}}\n";
        let first = ask(Format::Jsonl);
        let Held::Cut(rows, next) = held(&first, body) else {
            panic!("cut")
        };
        assert_eq!(rows, 100_000);
        assert_eq!((next.part, next.cursor.as_deref()), (2, Some("c2")));
        assert!(
            next.path().contains("from=2026-10-01") && next.path().ends_with("cursor=c2"),
            "{}",
            next.path()
        );
        assert_eq!(
            note("f.jsonl", &first, &Held::Cut(rows, next.clone())),
            "Saved f.jsonl: the first 100,000 rows."
        );
        let last = "{\"manifest\":{\"complete\":true,\"rows\":12}}";
        assert_eq!(held(&next, last), Held::Whole(Some(12)));
        assert_eq!(
            note("f.jsonl", &next, &Held::Whole(Some(12))),
            "Saved f.jsonl: the last 12 rows."
        );
        assert_eq!(held(&first, "{\"a\":1}"), Held::Unknown);
        assert!(read_manifest("").is_none());
    }

    #[test]
    fn a_short_csv_file_is_whole() {
        let first = ask(Format::Csv);
        assert_eq!(held(&first, "a,b\n1,2\n"), Held::Whole(None));
        assert_eq!(
            note("f.csv", &first, &Held::Whole(None)),
            "Saved f.csv, with every row."
        );
        let long = "x\n".repeat(100_001);
        assert_eq!(held(&first, &long), Held::Unknown);
        assert_eq!(
            note("f.csv", &first, &Held::Unknown),
            "Saved f.csv. It may stop at 100,000 rows; its manifest is in Audit."
        );
        assert_eq!(
            failure("Gateway answered 501."),
            "This Nexus Gateway can't export these yet."
        );
        assert_eq!(
            failure("Gateway answered 403."),
            "Only owners and admins download files."
        );
    }
}
