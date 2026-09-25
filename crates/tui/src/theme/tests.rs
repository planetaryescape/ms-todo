use std::path::Path;

use ratatui::style::{Color, Style};

use super::config::load_from;
use super::*;

#[test]
fn the_built_in_themes_default_to_terminal_and_have_unique_names() {
    let names = names();
    assert_eq!(names[0], "terminal");
    assert_eq!(DEFAULT, "terminal");
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len());
    assert_eq!(
        names,
        [
            "terminal",
            "catppuccin-mocha",
            "catppuccin-latte",
            "gruvbox-dark",
            "gruvbox-light",
            "tokyo-night",
            "nord",
            "one-dark",
            "kanagawa",
            "night-owl",
            "cobalt2",
            "high-contrast"
        ]
    );
}

/// The struct literal already makes a missing role a compile error; this
/// checks the RGB themes set each one rather than leaving it to the
/// terminal, which would mix the terminal's colours into a fixed palette.
#[test]
fn every_rgb_theme_defines_every_role() {
    for theme in BUILTIN.iter().filter(|theme| theme.name != DEFAULT) {
        let mut palette = theme.palette.clone();
        for role in ROLES {
            let color = *palette.role_mut(role).expect("every role is a field");
            // `Reset` keeps each cell's colour under the cursor, on purpose.
            if *role != "selection_fg" || theme.name == "high-contrast" {
                assert!(
                    matches!(color, Color::Rgb(..)),
                    "{}: {role} is {color:?}",
                    theme.name
                );
            }
        }
    }
}

/// The terminal theme uses only what the terminal's own palette defines.
#[test]
fn the_terminal_theme_uses_ansi_colours_only() {
    let mut palette = BUILTIN[0].palette.clone();
    for role in ROLES {
        let color = *palette.role_mut(role).expect("a role");
        assert!(
            !matches!(color, Color::Rgb(..) | Color::Indexed(_)),
            "{role} is {color:?}"
        );
    }
}

fn luminance(color: Color) -> f64 {
    assert!(matches!(color, Color::Rgb(..)), "{color:?} isn't RGB");
    let Color::Rgb(r, g, b) = color else {
        return 0.0;
    };
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG 2.2's contrast ratio,
/// https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html.
fn contrast(one: Color, other: Color) -> f64 {
    let (a, b) = (luminance(one), luminance(other));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn high_contrast_meets_wcag_aaa() {
    let mut palette = builtin("high-contrast").expect("theme").palette.clone();
    let background = palette.background;
    for role in ROLES {
        if matches!(*role, "background" | "selection_bg" | "header_bar") {
            continue;
        }
        let color = *palette.role_mut(role).expect("a role");
        let ratio = contrast(color, background);
        assert!(ratio >= 7.0, "{role}: {ratio:.1}:1");
    }
    let text_on_selection = contrast(palette.selection_fg, palette.selection_bg);
    assert!(text_on_selection >= 7.0, "{text_on_selection:.1}:1");
}

#[test]
fn the_capability_comes_from_no_color_and_colorterm() {
    assert_eq!(
        Capability::detect(None, Some("truecolor")),
        Capability::Truecolor
    );
    assert_eq!(
        Capability::detect(None, Some("24bit")),
        Capability::Truecolor
    );
    assert_eq!(Capability::detect(None, None), Capability::Ansi256);
    assert_eq!(Capability::detect(None, Some("yes")), Capability::Ansi256);
    assert_eq!(
        Capability::detect(Some("1"), Some("truecolor")),
        Capability::Monochrome
    );
    // no-color.org: set but empty doesn't count.
    assert_eq!(Capability::detect(Some(""), None), Capability::Ansi256);
}

fn choice(name: &'static str, capability: Capability) -> ThemeChoice {
    ThemeChoice {
        builtin: builtin(name).expect("a built-in theme"),
        capability,
        ..ThemeChoice::default()
    }
}

#[test]
fn without_truecolor_rgb_becomes_the_nearest_256_colour_index() {
    let truecolor = choice("catppuccin-mocha", Capability::Truecolor).theme();
    assert_eq!(truecolor.overdue.fg, Some(Color::Rgb(0xf3, 0x8b, 0xa8)));
    let fallback = choice("catppuccin-mocha", Capability::Ansi256).theme();
    assert_eq!(
        fallback.overdue.fg,
        Some(Color::Indexed(ansi_colours::ansi256_from_rgb((
            0xf3, 0x8b, 0xa8
        ))))
    );
    // Every fixed palette comes down to 256 colours whole: no RGB left.
    for theme in BUILTIN.iter().filter(|theme| theme.name != DEFAULT) {
        let styles = choice(theme.name, Capability::Ansi256).theme();
        let debug = format!("{styles:?}");
        assert!(!debug.contains("Rgb"), "{}: {debug}", theme.name);
        assert!(debug.contains("Indexed"), "{}", theme.name);
    }
    // The terminal theme's colours are the terminal's either way.
    assert_eq!(
        choice("terminal", Capability::Ansi256).theme(),
        choice("terminal", Capability::Truecolor).theme()
    );
}

#[test]
fn no_color_uses_no_colour_whatever_the_theme() {
    for theme in &BUILTIN {
        let styles = choice(theme.name, Capability::Monochrome).theme();
        assert_eq!(styles, Theme::monochrome());
    }
    let mono = Theme::monochrome();
    let every: Vec<Style> = vec![
        mono.base,
        mono.text,
        mono.text_dim,
        mono.selection,
        mono.overdue,
        mono.important,
        mono.sync_failed,
        mono.banner_error,
        mono.cursor,
    ];
    for style in every {
        assert_eq!((style.fg, style.bg), (None, None), "{style:?}");
    }
    // Meaning is still there: overdue is bold, the selection reversed.
    assert_ne!(mono.overdue, mono.text);
    assert_ne!(mono.selection, mono.text);
}

#[test]
fn the_selection_is_a_background_not_an_inverted_block() {
    for theme in &BUILTIN {
        let styles = choice(theme.name, Capability::Truecolor).theme();
        assert!(styles.selection.bg.is_some(), "{}", theme.name);
        assert!(
            !styles
                .selection
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "{}",
            theme.name
        );
    }
}

fn config(contents: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.toml"), contents).expect("write config");
    dir
}

fn load_in(dir: &Path, flag: Option<&str>) -> Result<ThemeChoice, ThemeError> {
    load_from(&dir.join("config.toml"), flag, Capability::Truecolor)
}

#[test]
fn the_flag_wins_over_the_config_which_wins_over_the_default() {
    let empty = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        load_in(empty.path(), None).expect("load").builtin.name,
        "terminal"
    );
    let dir = config("[tui]\ntheme = \"nord\"\n");
    assert_eq!(
        load_in(dir.path(), None).expect("load").builtin.name,
        "nord"
    );
    assert_eq!(
        load_in(dir.path(), Some("gruvbox-dark"))
            .expect("load")
            .builtin
            .name,
        "gruvbox-dark"
    );
}

#[test]
fn an_unknown_theme_names_it_and_lists_the_themes() {
    let empty = tempfile::tempdir().expect("tempdir");
    let error = load_in(empty.path(), Some("solarized"))
        .expect_err("unknown")
        .to_string();
    assert!(error.contains("\"solarized\""), "{error}");
    assert!(error.contains("--theme"), "{error}");
    assert!(
        error.contains("terminal, catppuccin-mocha, catppuccin-latte, gruvbox-dark"),
        "{error}"
    );
    let dir = config("[tui]\ntheme = \"dracula\"\n");
    let error = load_in(dir.path(), None).expect_err("unknown").to_string();
    assert!(
        error.contains("\"dracula\"") && error.contains("tui.theme in"),
        "{error}"
    );
}

#[test]
fn colour_overrides_apply_on_top_of_the_theme() {
    let dir = config(
        "[tui]\ntheme = \"nord\"\n\n[tui.colors]\noverdue = \"#ff0000\"\nborder = \"dark-gray\"\ncursor = \"reset\"\n",
    );
    let choice = load_in(dir.path(), None).expect("load");
    let theme = choice.theme();
    assert_eq!(theme.overdue.fg, Some(Color::Rgb(0xff, 0, 0)));
    assert_eq!(theme.border.fg, Some(Color::DarkGray));
    // Unchanged roles keep the theme's colour.
    assert_eq!(theme.important.fg, Some(Color::Rgb(0xeb, 0xcb, 0x8b)));
}

#[test]
fn a_bad_colour_setting_names_the_key() {
    for (contents, wanted) in [
        (
            "[tui.colors]\noverdew = \"red\"\n",
            "tui.colors.overdew is not a role",
        ),
        (
            "[tui.colors]\noverdue = \"pinkish\"\n",
            "tui.colors.overdue = \"pinkish\" is not a colour",
        ),
        (
            "[tui.colors]\noverdue = 3\n",
            "tui.colors.overdue must be a string",
        ),
        ("[tui]\ncolours = 1\n", "tui.colours is not a setting"),
        ("[tui]\ntheme = 1\n", "tui.theme must be a string"),
        ("tui = 1\n", "tui must be a table"),
        ("[tui\n", "config.toml"),
    ] {
        let dir = config(contents);
        let error = load_in(dir.path(), None).expect_err(contents).to_string();
        assert!(error.contains(wanted), "{contents:?}: {error}");
    }
}

#[test]
fn saving_keeps_the_rest_of_the_file_and_its_comments() {
    let original = "# ms-todo settings\n\n[auth]\n# my own app\nclient_id = \"abc\" # trailing\n\n[tui]\n# the look\ntheme = \"nord\" # was nord\n\n[tui.colors]\noverdue = \"#ff0000\"\n";
    let dir = config(original);
    let path = dir.path().join("config.toml");
    save(&path, "tokyo-night").expect("save");
    let saved = std::fs::read_to_string(&path).expect("read");
    assert_eq!(saved, original.replace("\"nord\"", "\"tokyo-night\""));
    assert_eq!(
        load_in(dir.path(), None).expect("load").builtin.name,
        "tokyo-night"
    );
}

#[test]
fn saving_adds_a_tui_table_or_a_whole_file() {
    let dir = config("# just auth\n[auth]\nclient_id = \"abc\"\n");
    let path = dir.path().join("config.toml");
    save(&path, "nord").expect("save");
    let saved = std::fs::read_to_string(&path).expect("read");
    assert!(
        saved.starts_with("# just auth\n[auth]\nclient_id = \"abc\"\n"),
        "{saved}"
    );
    assert!(saved.contains("[tui]\ntheme = \"nord\"\n"), "{saved}");

    let empty = tempfile::tempdir().expect("tempdir");
    let path = empty.path().join("ms-todo").join("config.toml");
    save(&path, "nord").expect("save");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "[tui]\ntheme = \"nord\"\n"
    );
}

#[cfg(unix)]
#[test]
fn saving_through_a_link_rewrites_the_file_it_points_to() {
    let dotfiles = config("[auth]\nclient_id = \"abc\"\n");
    let real = dotfiles.path().join("config.toml");
    let home = tempfile::tempdir().expect("tempdir");
    let link = home.path().join("config.toml");
    std::os::unix::fs::symlink(&real, &link).expect("link");
    save(&link, "nord").expect("save");
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat")
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::read_to_string(&real)
            .expect("read")
            .contains("theme = \"nord\"")
    );
}

/// Every colour and modifier is the theme's: a raw one anywhere else in
/// the TUI would ignore the user's theme and `NO_COLOR`.
#[test]
fn no_raw_styles_outside_the_theme() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut pending = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path == src.join("theme") {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            // Tests may check the theme's colours on screen.
            let is_test = path.file_name().is_some_and(|name| name == "tests.rs");
            if path.extension().is_none_or(|ext| ext != "rs") || is_test {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            for (number, line) in text.lines().enumerate() {
                if line.contains("Color::") || line.contains("Modifier::") {
                    offenders.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "use a theme role instead:\n{}",
        offenders.join("\n")
    );
}
