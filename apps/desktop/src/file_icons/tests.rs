use super::*;

fn file(path: &str) -> SharedString {
    FileIcon::file(path).asset(true)
}

fn folder(path: &str, open: bool) -> SharedString {
    FileIcon::folder(path, open).asset(true)
}

#[test]
fn a_file_takes_its_languages_icon_from_its_extension() {
    assert_eq!(file("src/main.rs"), "file_icons/rust.svg");
    assert_eq!(file("app.py"), "file_icons/python.svg");
    assert_eq!(file("ui/view.tsx"), "file_icons/react_ts.svg");
    assert_eq!(file("C:\\code\\lib.go"), "file_icons/go.svg");
}

#[test]
fn a_whole_name_beats_its_extension() {
    assert_eq!(file("package.json"), "file_icons/nodejs.svg");
    assert_eq!(file("web/tsconfig.json"), "file_icons/tsconfig.svg");
    assert_eq!(file("README.md"), "file_icons/readme.svg");
    assert_eq!(file("AGENTS.md"), "file_icons/agent.svg");
    assert_eq!(file(".gitignore"), "file_icons/git.svg");
    assert_eq!(file("Dockerfile"), "file_icons/docker.svg");
    assert_eq!(file("Dockerfile.prod"), "file_icons/docker.svg");
}

#[test]
fn the_longest_extension_wins() {
    assert_eq!(file("types/lib.d.ts"), "file_icons/typescript-def.svg");
    assert_eq!(file("lib.test.ts"), "file_icons/test-ts.svg");
    assert_eq!(file("lib.ts"), "file_icons/typescript.svg");
    // A dotfile's name after its dot is an extension too, as in VS Code.
    assert_eq!(file(".data.json"), "file_icons/json.svg");
}

#[test]
fn a_parent_and_name_beats_the_name_alone() {
    assert_eq!(file(".config/graphqlrc"), "file_icons/graphql.svg");
    assert_eq!(file("graphqlrc"), "file_icons/file.svg");
}

#[test]
fn a_language_claims_what_the_theme_does_not_name() {
    let (extension, pair) = table::LANGUAGE_EXTENSIONS
        .iter()
        .find(|(extension, _)| find(table::FILE_EXTENSIONS, extension).is_none())
        .expect("a language extension the theme does not name");
    assert_eq!(FileIcon::file(&format!("x.{extension}")), FileIcon(*pair));
    let (name, pair) = table::LANGUAGE_NAMES
        .iter()
        .find(|(name, _)| find(table::FILE_NAMES, name).is_none() && extensions(name).next().is_none())
        .expect("a language file name the theme does not name");
    assert_eq!(FileIcon::file(name), FileIcon(*pair));
}

#[test]
fn names_match_in_any_case_and_an_unknown_file_is_a_plain_file() {
    assert_eq!(file("MAIN.RS"), "file_icons/rust.svg");
    assert_eq!(file("notes.unknown-extension"), "file_icons/file.svg");
    assert_eq!(file("noextension"), "file_icons/file.svg");
    assert_eq!(file(""), "file_icons/file.svg");
}

#[test]
fn a_folder_has_its_own_icon_open_and_closed() {
    assert_eq!(folder("repo/src", false), "file_icons/folder-src.svg");
    assert_eq!(folder("repo/src", true), "file_icons/folder-src-open.svg");
    assert_eq!(folder(".github", false), "file_icons/folder-github.svg");
    assert_eq!(folder(".github/workflows", false), "file_icons/folder-gh-workflows.svg");
    assert_eq!(folder("node_modules/", true), "file_icons/folder-node-open.svg");
    assert_eq!(folder("some-folder", false), "file_icons/folder.svg");
    assert_eq!(folder("some-folder", true), "file_icons/folder-open.svg");
}

#[test]
fn a_light_theme_shows_the_light_variant_where_the_theme_has_one() {
    assert_eq!(FileIcon::file("Cargo.toml").asset(true), "file_icons/toml.svg");
    assert_eq!(FileIcon::file("Cargo.toml").asset(false), "file_icons/toml_light.svg");
    assert_eq!(FileIcon::file("main.rs").asset(false), "file_icons/rust.svg");
}

#[test]
fn every_table_is_sorted_and_every_icon_loads() {
    fn sorted<'a>(keys: impl Iterator<Item = &'a str>) -> bool {
        keys.collect::<Vec<_>>().windows(2).all(|pair| pair[0] < pair[1])
    }
    assert!(sorted(table::ICONS.iter().map(|(path, _)| *path)));
    for names in [
        table::FILE_NAMES,
        table::FILE_EXTENSIONS,
        table::LANGUAGE_NAMES,
        table::LANGUAGE_EXTENSIONS,
    ] {
        assert!(sorted(names.iter().map(|(key, _)| *key)));
        assert!(names.iter().all(|(key, _)| *key == key.to_lowercase()));
    }
    assert!(sorted(table::FOLDER_NAMES.iter().map(|(key, _, _)| *key)));
    for (path, svg) in table::ICONS {
        assert!(
            std::str::from_utf8(svg).is_ok_and(|svg| svg.trim_start().starts_with("<svg")),
            "{path}"
        );
        assert_eq!(load(path).as_deref(), Some(*svg));
    }
    assert!(load("file_icons/not-an-icon.svg").is_none());
}
