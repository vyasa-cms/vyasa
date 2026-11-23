use super::layout::place;
use super::*;

const CARD_FONT: &[u8] = include_bytes!("../../testdata/social_card/noto-sans-semibold-card.ttf");
const ASCII_FONT: &[u8] = include_bytes!("../../testdata/social_card/noto-ascii.ttf");
const NO_ELLIPSIS_FONT: &[u8] =
    include_bytes!("../../testdata/social_card/noto-ascii-no-ellipsis.ttf");
const NO_DOTS_FONT: &[u8] = include_bytes!("../../testdata/social_card/noto-ascii-no-dots.ttf");
const CJK_FONT: &[u8] = include_bytes!("../../testdata/social_card/noto-sans-sc-subset.ttf");

fn layout() -> CardLayout {
    CardLayout {
        post_types: vec!["docs".to_owned()],
        background: "images/og/card-bg.png".to_owned(),
        font: "fonts/card.ttf".to_owned(),
        color: "#F5F5F4".to_owned(),
        label_color: Some("#A8A29E".to_owned()),
        title: TitleBox {
            x: 80,
            y: 200,
            w: 1040,
            h: 300,
            size: 72,
            min_size: 44,
            max_lines: 3,
        },
        label: Some(LabelBox {
            x: 80,
            y: 120,
            w: 1040,
            size: 32,
        }),
        alt: "{title}: {site}".to_owned(),
    }
}

fn background_png(rgb: [u8; 3]) -> Vec<u8> {
    let img = RgbaImage::from_pixel(WIDTH, HEIGHT, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn theme_with(layout: CardLayout, font: &[u8]) -> CardTheme {
    CardTheme::load(layout, "section", &background_png([28, 25, 23]), font).unwrap()
}

fn font(bytes: &[u8]) -> FontVec {
    FontVec::try_from_vec(bytes.to_vec()).unwrap()
}

// ---- cleaning --------------------------------------------------------

#[test]
fn clean_removes_soft_hyphen_and_zero_width_space() {
    assert_eq!(clean("Hash\u{AD}Join\u{200B}s", 200), "HashJoins");
}

#[test]
fn clean_keeps_a_word_break_for_tab_and_newline() {
    assert_eq!(clean("Hash\tjoin\nsteps", 200), "Hash join steps");
}

#[test]
fn clean_gives_one_space_for_a_format_char_between_spaces() {
    assert_eq!(clean("a \u{200B} b", 200), "a b");
}

#[test]
fn clean_collapses_no_break_spaces_and_trims() {
    assert_eq!(clean("\u{A0} a\u{A0}\u{A0}b \u{2003}", 200), "a b");
}

#[test]
fn clean_applies_nfc_before_the_cut() {
    assert_eq!(clean("Cafe\u{301}", 200), "Caf\u{E9}");
    assert_eq!(clean("e\u{301}e\u{301}", 1), "\u{E9}");
}

#[test]
fn clean_cuts_at_200_characters_and_trims_after_the_cut() {
    let long = "x".repeat(250);
    assert_eq!(clean(&long, MAX_TITLE_CHARS).chars().count(), 200);
    let spaced = format!("{} tail", "y".repeat(199));
    assert_eq!(clean(&spaced, 200), "y".repeat(199));
}

#[test]
fn clean_keeps_markup_and_entities_as_written() {
    assert_eq!(clean("ARRAY<INT> & more", 200), "ARRAY<INT> & more");
}

#[test]
fn clean_of_only_format_characters_is_empty() {
    assert_eq!(clean("\u{200B}\u{AD}\u{FEFF}", 200), "");
}

// ---- text check ------------------------------------------------------

#[test]
fn text_check_accepts_latin_greek_cyrillic_and_punctuation() {
    let t = theme_with(layout(), CARD_FONT);
    assert!(t.supports("SQL: ARRAY<INT> & more (α, Ж) — 100%", "SQL reference"));
}

#[test]
fn text_check_refuses_scripts_outside_the_allow_list() {
    let t = theme_with(layout(), CARD_FONT);
    assert!(!t.supports("مرحبا", "Docs"), "Arabic");
    assert!(!t.supports("नमस्ते", "Docs"), "Devanagari");
    assert!(!t.supports("שלום", "Docs"), "Hebrew");
}

#[test]
fn text_check_refuses_a_lone_combining_mark_and_a_missing_glyph() {
    let t = theme_with(layout(), CARD_FONT);
    assert!(!t.supports("a\u{301}", "Docs"), "combining mark");
    assert!(
        !t.supports("Join \u{2603}", "Docs"),
        "snowman not in the font"
    );
    assert!(!t.supports("Join", "Snow \u{2603}"), "label checked too");
}

#[test]
fn a_decomposed_e_acute_is_drawn_after_nfc() {
    let t = theme_with(layout(), CARD_FONT);
    let title = clean("Cafe\u{301}", MAX_TITLE_CHARS);
    assert!(t.supports(&title, "Docs"));
}

#[test]
fn an_empty_title_has_no_card() {
    let t = theme_with(layout(), CARD_FONT);
    assert!(!t.supports("", "Docs"));
}

// ---- layout ----------------------------------------------------------

#[test]
fn a_short_title_takes_one_line_at_full_size() {
    let f = font(CARD_FONT);
    let p = place(&f, &layout(), "Joins", "SQL reference").unwrap();
    assert_eq!(p.title, vec!["Joins".to_owned()]);
    assert_eq!(p.title_size, 72);
    assert_eq!(p.label.as_deref(), Some("SQL reference"));
}

#[test]
fn a_longer_title_wraps_at_spaces() {
    let f = font(CARD_FONT);
    let title = "How the cost-based optimizer chooses a join order for star schemas";
    let p = place(&f, &layout(), title, "").unwrap();
    assert!(p.title.len() > 1 && p.title.len() <= 3, "{:?}", p.title);
    assert_eq!(p.title.join(" "), title);
    assert_eq!(p.label, None, "an empty label draws none");
}

#[test]
fn step_down_is_clamped_at_min_size() {
    // size 70, min 44: 70 -> 66 -> ... -> 46 -> 44. Find the narrowest box
    // the run fits whole: that size is 44, never 42.
    let f = font(ASCII_FONT);
    let mut l = layout();
    l.title.size = 70;
    l.title.max_lines = 1;
    let fits_whole = |l: &CardLayout| {
        let p = place(&f, l, "spark.sql.shuffle", "").unwrap();
        (p.title.len() == 1 && !p.title[0].ends_with('…')).then_some(p.title_size)
    };
    l.title.w = 100;
    while fits_whole(&l).is_none() {
        l.title.w += 1;
        assert!(l.title.w < 1040, "never fit");
    }
    assert_eq!(fits_whole(&l), Some(44));
    l.title.w -= 1;
    let p = place(&f, &l, "spark.sql.shuffle", "").unwrap();
    assert_eq!(p.title_size, 44, "never below min_size");
}

#[test]
fn a_long_single_word_steps_down_before_it_is_broken() {
    let f = font(CARD_FONT);
    let mut l = layout();
    l.title.w = 900;
    let p = place(&f, &l, "spark.sql.autoBroadcastJoinThreshold", "").unwrap();
    assert_eq!(p.title.len(), 1, "{:?}", p.title);
    assert!(p.title_size < 72);
    assert_eq!(p.title[0], "spark.sql.autoBroadcastJoinThreshold");
}

#[test]
fn an_over_wide_run_at_min_size_is_broken_at_characters() {
    let f = font(CARD_FONT);
    let mut l = layout();
    l.title.w = 300;
    let p = place(&f, &l, "spark.sql.autoBroadcastJoinThreshold", "").unwrap();
    assert_eq!(p.title_size, 44);
    assert!(p.title.len() > 1);
    assert!(p.title.concat().starts_with("spark.sql.auto"));
}

#[test]
fn a_cjk_title_wraps_between_characters() {
    let f = font(CJK_FONT);
    let title = "数据处理引擎的查询优化器与执行计划分析指南手册第一章节概述数据处理";
    assert_eq!(title.chars().count(), 33);
    let p = place(&f, &layout(), title, "").unwrap();
    assert!(p.title.len() > 1, "{:?}", p.title);
    assert!(!p.title.iter().any(|l| l.contains(' ')));
    let t = theme_with(layout(), CJK_FONT);
    assert!(t.supports(title, ""));
}

#[test]
fn a_short_box_limits_lines_below_max_lines() {
    let f = font(CARD_FONT);
    let mut l = layout();
    l.title.h = 120; // floor(5 x 120 / (6 x 44)) = 2 lines at min_size
    l.title.max_lines = 3;
    let title = "word ".repeat(60);
    let p = place(&f, &l, title.trim(), "").unwrap();
    assert_eq!(p.title.len(), 2);
    assert!(p.title[1].ends_with('…'), "{:?}", p.title);
    assert_is_a_cut_of(&p.title, title.trim());
}

#[test]
fn the_last_line_ends_in_an_ellipsis_cut_at_a_space() {
    let f = font(CARD_FONT);
    let title = "Spark SQL ".repeat(20);
    let p = place(&f, &layout(), title.trim(), "").unwrap();
    assert_eq!(p.title.len(), 3);
    assert_eq!(p.title_size, 44);
    let last = &p.title[2];
    assert!(last.ends_with('…'));
    let body = last.trim_end_matches('…');
    assert!(body.ends_with("SQL") || body.ends_with("Spark"), "{last}");
    assert_is_a_cut_of(&p.title, title.trim());
}

/// The drawn lines, read in order without the ellipsis, are the start of
/// the title: no line repeats words or runs on past the cut.
fn assert_is_a_cut_of(lines: &[String], title: &str) {
    let drawn = lines.join(" ");
    let drawn = drawn.trim_end_matches('…');
    assert!(
        title.starts_with(drawn),
        "{lines:?} is not a cut of {title:?}"
    );
}

#[test]
fn a_cut_title_keeps_each_word_once_and_every_line_in_the_box() {
    let f = font(CARD_FONT);
    let title = "A very long page title that keeps going well past what three lines of the card \
                 can hold, so the renderer has to cut it and end the last line with an ellipsis";
    let p = place(&f, &layout(), title, "").unwrap();
    assert_eq!(p.title.len(), 3, "{:?}", p.title);
    assert!(p.title[2].ends_with('…'), "{:?}", p.title);
    assert_is_a_cut_of(&p.title, title);
}

#[test]
fn without_an_ellipsis_glyph_the_cut_uses_three_full_stops() {
    let f = font(NO_ELLIPSIS_FONT);
    let title = "Spark SQL ".repeat(20);
    let p = place(&f, &layout(), title.trim(), "").unwrap();
    assert!(p.title[2].ends_with("..."), "{:?}", p.title);
}

#[test]
fn without_ellipsis_and_full_stop_a_cut_title_has_no_card() {
    let t = theme_with(layout(), NO_DOTS_FONT);
    let title = "Spark SQL ".repeat(20);
    assert!(!t.supports(title.trim(), ""));
    assert!(t.supports("Joins", ""), "no cut, no ellipsis needed");
}

#[test]
fn a_long_label_is_cut_on_one_line() {
    let f = font(CARD_FONT);
    let mut l = layout();
    l.label = Some(LabelBox {
        x: 80,
        y: 120,
        w: 200,
        size: 32,
    });
    let p = place(&f, &l, "Joins", "Structured Query Language reference").unwrap();
    let label = p.label.unwrap();
    assert!(label.ends_with('…'), "{label}");
}

#[test]
fn a_200_character_title_is_cut_and_still_drawn() {
    let t = theme_with(layout(), CARD_FONT);
    let title = clean(&"Partition pruning ".repeat(20), MAX_TITLE_CHARS);
    assert_eq!(title.chars().count(), 200);
    let png = t.render(&title, "SQL reference").unwrap();
    assert_eq!(png_dimensions(&png), Some((WIDTH, HEIGHT)));
}

// ---- drawing ---------------------------------------------------------

fn decode(png: &[u8]) -> RgbaImage {
    image::load_from_memory(png).unwrap().to_rgba8()
}

#[test]
fn render_draws_text_inside_the_boxes_only() {
    let t = theme_with(layout(), CARD_FONT);
    let png = t.render("Joins and aggregates", "SQL reference").unwrap();
    let img = decode(&png);
    assert_eq!(img.dimensions(), (WIDTH, HEIGHT));
    let bg = image::Rgba([28, 25, 23, 255]);
    let mut changed = 0;
    for (x, y, p) in img.enumerate_pixels() {
        if *p != bg {
            changed += 1;
            assert!((60..=1140).contains(&x), "pixel at x={x}");
            assert!((100..=520).contains(&y), "pixel at y={y}");
        }
    }
    assert!(changed > 1000, "text was drawn");
}

#[test]
fn drawing_near_the_edge_clips_and_does_not_panic() {
    let mut l = layout();
    l.title = TitleBox {
        x: 1100,
        y: 560,
        w: 100,
        h: 70,
        size: 44,
        min_size: 44,
        max_lines: 1,
    };
    l.label = Some(LabelBox {
        x: 1150,
        y: 0,
        w: 50,
        size: 40,
    });
    let t = theme_with(l, CARD_FONT);
    let png = t.render("Wide words go here", "Label text").unwrap();
    assert_eq!(decode(&png).dimensions(), (WIDTH, HEIGHT));
}

#[test]
fn render_is_deterministic() {
    let t = theme_with(layout(), CARD_FONT);
    let a = t.render("Window functions", "SQL reference").unwrap();
    let b = t.render("Window functions", "SQL reference").unwrap();
    assert_eq!(a, b);
}

// ---- key -------------------------------------------------------------

#[test]
fn key_fields_are_length_prefixed() {
    assert_ne!(card_key(&[b"a", b"bc"]), card_key(&[b"ab", b"c"]));
    assert_eq!(card_key(&[b"a"]).len(), 16);
}

#[test]
fn key_changes_with_the_font_file_at_the_same_theme_version() {
    let a = theme_with(layout(), CARD_FONT);
    let b = theme_with(layout(), ASCII_FONT);
    assert_ne!(
        a.key("acme", 3, "Joins", "SQL"),
        b.key("acme", 3, "Joins", "SQL")
    );
    assert_eq!(
        a.key("acme", 3, "Joins", "SQL"),
        theme_with(layout(), CARD_FONT).key("acme", 3, "Joins", "SQL")
    );
}

#[test]
fn key_covers_the_whole_title_and_the_section_text() {
    let a = theme_with(layout(), CARD_FONT);
    assert_ne!(
        a.key("t", 1, "ARRAY<INT> & more", ""),
        a.key("t", 1, "ARRAY<INT>", "")
    );
    let other = CardTheme::load(
        layout(),
        "section2",
        &background_png([28, 25, 23]),
        CARD_FONT,
    )
    .unwrap();
    assert_ne!(a.key("t", 1, "x", ""), other.key("t", 1, "x", ""));
}

// ---- alt -------------------------------------------------------------

#[test]
fn alt_fills_placeholders_in_one_pass() {
    let mut l = layout();
    l.alt = "{title} ({label}) on {site}".to_owned();
    assert_eq!(
        l.alt_text("A {site} B", "Docs", "Acme"),
        "A {site} B (Docs) on Acme"
    );
}

// ---- manifest validation ---------------------------------------------

fn invalid(f: impl FnOnce(&mut CardLayout)) -> String {
    let mut l = layout();
    f(&mut l);
    l.validate().unwrap_err()
}

#[test]
fn the_example_layout_is_valid() {
    layout().validate().unwrap();
}

#[test]
fn validation_refuses_each_broken_rule() {
    assert!(invalid(|l| l.post_types.clear()).contains("post_types"));
    assert!(invalid(|l| l.background = "../x.png".to_owned()).contains("background"));
    assert!(invalid(|l| l.background = "/images/x.png".to_owned()).contains("background"));
    assert!(invalid(|l| l.background = "images/../x.png".to_owned()).contains("background"));
    assert!(invalid(|l| l.background = "images/x.jpg".to_owned()).contains("background"));
    assert!(invalid(|l| l.font = "fonts/x.woff2".to_owned()).contains("font"));
    assert!(invalid(|l| l.color = "red".to_owned()).contains("color"));
    assert!(invalid(|l| l.label_color = Some("#12345".to_owned())).contains("label_color"));
    assert!(invalid(|l| l.title.min_size = 0).contains("min_size"));
    assert!(invalid(|l| l.title.min_size = 80).contains("min_size"));
    assert!(invalid(|l| l.title.size = 201).contains("size"));
    assert!(invalid(|l| l.title.max_lines = 0).contains("max_lines"));
    assert!(invalid(|l| l.title.x = 200).contains("inside"));
    assert!(invalid(|l| l.title.y = 400).contains("inside"));
    assert!(invalid(|l| l.title.h = 52).contains("one line"));
    assert!(invalid(|l| l.title.w = 40).contains("one line"));
    assert!(invalid(|l| l.label.as_mut().unwrap().size = 0).contains("label"));
    assert!(invalid(|l| l.label.as_mut().unwrap().y = 600).contains("inside"));
    assert!(invalid(|l| l.label.as_mut().unwrap().w = 20).contains("wide"));
    assert!(invalid(|l| l.alt = "x".repeat(301)).contains("alt"));
    assert!(invalid(|l| l.alt = "{author}".to_owned()).contains("alt"));
}

#[test]
fn the_title_height_rule_is_the_integer_form() {
    // 6 x 44 = 264 <= 5 x 53 = 265, so h = 53 holds one line at 44.
    let mut l = layout();
    l.title.h = 53;
    l.validate().unwrap();
}

#[test]
fn background_must_be_a_1200x630_png() {
    let small = {
        let img = RgbaImage::new(600, 315);
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    };
    let e = CardTheme::load(layout(), "s", &small, CARD_FONT).unwrap_err();
    assert!(e.contains("600x315"), "{e}");
    let e = CardTheme::load(layout(), "s", b"GIF89a....", CARD_FONT).unwrap_err();
    assert!(e.contains("not a PNG"), "{e}");
    let big = vec![0u8; MAX_BACKGROUND_BYTES + 1];
    assert!(CardTheme::load(layout(), "s", &big, CARD_FONT)
        .unwrap_err()
        .contains("maximum"));
}

#[test]
fn font_must_parse_and_stay_under_the_cap() {
    let bg = background_png([0, 0, 0]);
    let e = CardTheme::load(layout(), "s", &bg, b"not a font").unwrap_err();
    assert!(e.contains("font"), "{e}");
    let big = vec![0u8; MAX_FONT_BYTES + 1];
    assert!(CardTheme::load(layout(), "s", &bg, &big)
        .unwrap_err()
        .contains("maximum"));
}

#[test]
fn the_stored_section_refuses_unknown_keys() {
    let mut v = serde_json::to_value(layout()).unwrap();
    serde_json::from_value::<CardLayout>(v.clone()).unwrap();
    v["subtitle"] = serde_json::json!("x");
    assert!(serde_json::from_value::<CardLayout>(v).is_err());
}
