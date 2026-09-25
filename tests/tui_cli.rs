//! `ms-todo tui`'s theme flags, which answer before the daemon or the
//! terminal is touched, so they run anywhere.

mod support;

use support::Env;

#[test]
fn list_themes_prints_every_name_default_first() {
    let env = Env::new();
    let output = env
        .cmd()
        .args(["tui", "--list-themes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(output).expect("utf8"),
        "terminal\ncatppuccin-mocha\ncatppuccin-latte\ngruvbox-dark\ngruvbox-light\ntokyo-night\nnord\none-dark\nkanagawa\nnight-owl\ncobalt2\nhigh-contrast\n"
    );
}

#[test]
fn an_unknown_theme_is_a_usage_error_listing_the_themes() {
    let env = Env::new();
    let output = env
        .cmd()
        .args(["tui", "--theme", "solarized"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("unknown theme") && stderr.contains("solarized"),
        "{stderr}"
    );
    assert!(stderr.contains("catppuccin-mocha"), "{stderr}");
}

#[test]
fn a_bad_colour_in_config_names_the_key() {
    let env = Env::new();
    let config = env.home.path().join("config").join("ms-todo");
    std::fs::create_dir_all(&config).expect("config dir");
    std::fs::write(
        config.join("config.toml"),
        "[tui.colors]\noverdue = \"pinkish\"\n",
    )
    .expect("write config");
    let output = env.cmd().arg("tui").assert().code(2).get_output().clone();
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("tui.colors.overdue"), "{stderr}");
}

#[test]
fn signed_out_benchmark_exits_with_the_instance_login_command() {
    let env = Env::new();
    let error = env.failure(&["tui", "--bench-startup"], 4);
    assert_eq!(error["error"]["kind"], "auth_required");
    assert!(
        error["error"]["message"]
            .as_str()
            .expect("message")
            .contains("ms-todo --instance dev auth login")
    );
}
