//! .vytheme package tests: the blog starter installs cleanly; hostile or
//! broken packages are rejected with author-friendly errors.

#![allow(clippy::pedantic)]

use std::io::Write as _;
use std::path::Path;

use vyasa_themes::package::parse_vytheme;

const STARTER_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../themes-starter/blog");

fn read(rel: &[&str]) -> Vec<u8> {
    let mut path = std::path::PathBuf::from(STARTER_DIR);
    for seg in rel {
        path.push(seg);
    }
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Zips files into an in-memory archive.
fn zip_files(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, data) in entries {
            if w.start_file(name.clone(), zip::write::SimpleFileOptions::default())
                .is_err()
            {
                unreachable!("zip writer rejected {name}");
            }
            if w.write_all(data).is_err() {
                unreachable!("zip writer failed on {name}");
            }
        }
        if w.finish().is_err() {
            unreachable!("zip finish failed");
        }
    }
    buf.into_inner()
}

fn blog_entries() -> Vec<(String, Vec<u8>)> {
    vec![
        ("manifest.toml".into(), read(&["manifest.toml"])),
        ("tokens.json".into(), read(&["tokens.json"])),
        ("layout.json".into(), read(&["layout.json"])),
        (
            "templates/single.tera".into(),
            read(&["templates", "single.tera"]),
        ),
    ]
}

#[test]
fn blog_starter_installs() {
    let parsed = parse_vytheme(&zip_files(&blog_entries())).expect("blog package is valid");
    assert_eq!(parsed.manifest.name, "blog");
    // Pinned deliberately: a starter's package version is what decides
    // whether an installed site is offered an update, so bumping it
    // should be a conscious edit here too.
    assert_eq!(parsed.manifest.version, 5);
    assert_eq!(parsed.manifest.required_api, 1);
    assert_eq!(parsed.tokens_json["typography"]["heading"], "serif");
    assert!(parsed.layout_json.get("index").is_some());
    let single = parsed
        .templates
        .get("single.html")
        .expect("override registered");
    assert!(
        single.starts_with("{% extends \"base.html\" %}") && single.contains("blog-single"),
        "single.tera override registered under its template name"
    );
    assert!(!parsed.templates.contains_key("single.tera"));
    assert!(parsed.screenshot_png.is_none());
}

#[test]
fn screenshot_is_installed_when_present_and_png() {
    let mut entries = blog_entries();
    // Minimal valid PNG header + payload.
    let png: Vec<u8> = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0u8, 16))
        .collect();
    entries.push(("screenshot.png".into(), png));
    let parsed = parse_vytheme(&zip_files(&entries)).expect("valid");
    assert!(parsed.screenshot_png.is_some());
}

#[test]
fn bad_zips_are_rejected() {
    assert!(parse_vytheme(b"not a zip at all")
        .expect_err("garbage rejected")
        .to_string()
        .contains("not a valid"));
    // Valid zip but empty.
    let empty = zip_files(&[]);
    let err = parse_vytheme(&empty).expect_err("empty rejected");
    assert!(err.to_string().contains("manifest.toml"), "{err}");
}

#[test]
fn path_traversal_is_rejected() {
    let mut entries = blog_entries();
    entries.insert(0, ("../evil.txt".into(), b"x".to_vec()));
    let err = parse_vytheme(&zip_files(&entries)).expect_err("traversal rejected");
    assert!(err.to_string().contains("unsafe entry path"), "{err}");

    let mut entries = blog_entries();
    entries.insert(0, ("/abs/path.txt".into(), b"x".to_vec()));
    assert!(parse_vytheme(&zip_files(&entries)).is_err());

    let mut entries = blog_entries();
    entries.push(("templates/../steal.tera".into(), b"{}".to_vec()));
    assert!(parse_vytheme(&zip_files(&entries)).is_err());
}

#[test]
fn duplicate_entries_are_rejected() {
    // The zip writer refuses duplicates itself, so exercise the parser's
    // rule through its public helper.
    let mut names: Vec<String> = blog_entries().into_iter().map(|(n, _)| n).collect();
    let first = names[0].clone();
    names.push(first);
    let err = vyasa_themes::package::check_entry_names(&names).expect_err("dupes rejected");
    assert!(err.to_string().contains("duplicate entry"), "{err}");
}

#[test]
fn invalid_tokens_are_rejected_with_field_paths() {
    let mut entries = blog_entries();
    for e in &mut entries {
        if e.0 == "tokens.json" {
            e.1 = br#"{"version": 2}"#.to_vec();
        }
    }
    let err = parse_vytheme(&zip_files(&entries)).expect_err("bad tokens rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("tokens.json") && msg.contains("version"),
        "{msg}"
    );

    let mut entries = blog_entries();
    for e in &mut entries {
        if e.0 == "tokens.json" {
            e.1 = String::from(r##"{"colors": {"bg": {"light": "#zzz"}}}"##).into_bytes();
        }
    }
    assert!(
        parse_vytheme(&zip_files(&entries)).is_err(),
        "bad hex rejected"
    );
}

#[test]
fn invalid_layout_is_rejected() {
    let mut entries = blog_entries();
    for e in &mut entries {
        if e.0 == "layout.json" {
            e.1 = br#"{"index": [{"id": "x", "kind": "no-such-block"}]}"#.to_vec();
        }
    }
    let err = parse_vytheme(&zip_files(&entries)).expect_err("bad layout rejected");
    assert!(err.to_string().contains("layout.json"), "{err}");
}

#[test]
fn broken_tera_is_rejected_at_install() {
    let mut entries = blog_entries();
    for e in &mut entries {
        if e.0 == "templates/single.tera" {
            e.1 = br#"{% extends "base.html" %}{% block body %}{% endblock"#.to_vec();
        }
    }
    let err = parse_vytheme(&zip_files(&entries)).expect_err("broken tera rejected");
    let msg = err.to_string();
    assert!(msg.contains("templates/single.tera"), "{msg}");

    // Sandbox escape through a theme template.
    let mut entries = blog_entries();
    for e in &mut entries {
        if e.0 == "templates/single.tera" {
            e.1 = br#"{{ get_env(name="HOME") }}"#.to_vec();
        }
    }
    assert!(
        parse_vytheme(&zip_files(&entries)).is_err(),
        "escape rejected"
    );
}

#[test]
fn manifest_rules_are_enforced() {
    let with_manifest = |manifest: String| {
        let mut entries = blog_entries();
        for e in &mut entries {
            if e.0 == "manifest.toml" {
                e.1 = manifest.clone().into_bytes();
            }
        }
        zip_files(&entries)
    };
    let manifest_with = |name: &str, version: u32, api: u32| {
        format!("name = \"{name}\"\nversion = {version}\nauthor = \"T\"\nrequired_api = {api}\n")
    };
    // Wrong API level.
    let err = parse_vytheme(&with_manifest(manifest_with("blog", 1, 9))).expect_err("api mismatch");
    assert!(err.to_string().contains("required_api"), "{err}");
    // Version zero.
    let err = parse_vytheme(&with_manifest(manifest_with("blog", 0, 1))).expect_err("v0 rejected");
    assert!(err.to_string().contains("version must be >= 1"), "{err}");
    // Bad name.
    let err =
        parse_vytheme(&with_manifest(manifest_with("Bad Name!", 1, 1))).expect_err("name rejected");
    assert!(err.to_string().contains("name"), "{err}");
}

#[test]
fn starter_directory_is_present() {
    // Guard against accidental edits that move the shipped theme.
    assert!(Path::new(STARTER_DIR).join("manifest.toml").exists());
}

// ---- bundled pictures and fonts (phase 70) ----------------------------

fn png_bytes() -> Vec<u8> {
    [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0u8, 16))
        .collect()
}

#[test]
fn bundled_images_and_fonts_are_read_with_their_types() {
    let mut entries = blog_entries();
    entries.push(("assets/images/hero.jpg".into(), vec![0xFF, 0xD8, 0xFF, 0]));
    entries.push((
        "assets/images/icons/mark.svg".into(),
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
    ));
    entries.push(("assets/fonts/serif.woff2".into(), b"wOF2".to_vec()));
    entries.push((
        "assets/theme.css".into(),
        b".x{background:url(images/hero.jpg)}".to_vec(),
    ));
    let parsed = parse_vytheme(&zip_files(&entries)).expect("valid");
    let mut got: Vec<(String, &str)> = parsed
        .files
        .iter()
        .map(|f| (f.path.clone(), f.content_type))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("fonts/serif.woff2".to_owned(), "font/woff2"),
            ("images/hero.jpg".to_owned(), "image/jpeg"),
            ("images/icons/mark.svg".to_owned(), "image/svg+xml"),
        ]
    );
    // The text assets still travel on the row, untouched by the files.
    assert!(parsed.assets_json.is_some());
}

#[test]
fn a_file_outside_images_or_fonts_is_refused_by_name() {
    for (name, needle) in [
        ("assets/hero.jpg", "unexpected file"),
        ("assets/scripts/x.js", "unexpected file"),
        ("assets/images/Hero.JPG", "unexpected file"), // uppercase
        ("assets/images/.hidden.png", "unexpected file"),
        ("assets/images/a/b/c/d.png", "unexpected file"), // too deep
        ("assets/images/page.html", "images may be"),
        ("assets/fonts/font.exe", "fonts may be"),
        ("assets/images/mark.svg", "does not look like an SVG"),
    ] {
        let mut entries = blog_entries();
        entries.push((name.into(), b"MZ not an svg".to_vec()));
        let e = parse_vytheme(&zip_files(&entries)).expect_err(name);
        assert!(e.0.contains(needle), "{name}: {e}");
    }
}

#[test]
fn bundled_files_are_bounded_in_total() {
    // Five entries just under the per-entry cap sum past the total cap.
    let mut entries = blog_entries();
    let big = vec![0u8; vyasa_themes::package::MAX_ENTRY_BYTES - 1];
    for i in 0..5 {
        entries.push((format!("assets/images/{i}.png"), big.clone()));
    }
    let e = parse_vytheme(&zip_files(&entries)).expect_err("too much");
    assert!(e.0.contains("inflate past"), "{e}");
}

#[test]
fn a_bundled_font_may_be_declared_in_tokens() {
    let mut entries = blog_entries();
    let tokens = r#"{"version":1,"typography":{"heading":{"custom":"Fraunces"},
        "font_faces":[{"family":"Fraunces","src":"/theme-assets/fonts/fraunces.woff2"}]}}"#;
    entries.retain(|(n, _)| n != "tokens.json");
    entries.push(("tokens.json".into(), tokens.as_bytes().to_vec()));
    entries.push(("assets/fonts/fraunces.woff2".into(), b"wOF2".to_vec()));
    parse_vytheme(&zip_files(&entries)).expect("a bundled font source is allowed");

    // Anything else that is not https:// is still refused: a font face is
    // inlined into a <style> element and must stay a URL.
    let bad = tokens.replace(
        "/theme-assets/fonts/fraunces.woff2",
        "/media/fraunces.woff2",
    );
    entries.retain(|(n, _)| n != "tokens.json");
    entries.push(("tokens.json".into(), bad.into_bytes()));
    let e = parse_vytheme(&zip_files(&entries)).expect_err("other site paths are not fonts");
    assert!(e.0.contains("/theme-assets/fonts/"), "{e}");
}

#[test]
fn screenshot_and_bundled_files_coexist() {
    let mut entries = blog_entries();
    entries.push(("screenshot.png".into(), png_bytes()));
    entries.push(("assets/images/hero.png".into(), png_bytes()));
    let parsed = parse_vytheme(&zip_files(&entries)).expect("valid");
    assert!(parsed.screenshot_png.is_some());
    assert_eq!(parsed.files.len(), 1);
}
