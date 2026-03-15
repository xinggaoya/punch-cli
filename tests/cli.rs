use std::fs;

use clap::Parser;
use punch::cli::{Cli, Commands};
use punch::config::PunchConfig;
use punch::types::{LocalProtocol, TunnelTarget};
use tempfile::tempdir;

#[test]
fn parse_target_with_default_port() {
    let target = TunnelTarget::parse("demo.com").expect("target should parse");
    assert_eq!(target.domain, "demo.com");
    assert_eq!(target.port, 8080);
}

#[test]
fn parse_target_with_explicit_port() {
    let target = TunnelTarget::parse("api.example.com:3000").expect("target should parse");
    assert_eq!(target.domain, "api.example.com");
    assert_eq!(target.port, 3000);
}

#[test]
fn cli_parses_top_level_target_and_flags() {
    let cli = Cli::try_parse_from(["punch", "demo.com:8443", "--https", "--detach"])
        .expect("cli should parse");
    assert_eq!(cli.target.as_deref(), Some("demo.com:8443"));
    assert_eq!(cli.selected_protocol(), Some(LocalProtocol::Https));
    assert!(cli.detach);
}

#[test]
fn cli_parses_subcommands() {
    let cli = Cli::try_parse_from(["punch", "stop", "demo.com"]).expect("cli should parse");
    match cli.command {
        Some(Commands::Stop { domain }) => assert_eq!(domain, "demo.com"),
        _ => panic!("expected stop command"),
    }
}

#[test]
fn punch_config_loads_yaml() {
    let dir = tempdir().expect("tempdir should exist");
    let config_path = dir.path().join("punch.yml");
    fs::write(
        &config_path,
        r#"
tunnels:
  - domain: api.example.com
    port: 3000
    https: true
  - domain: demo.com
    port: 8080
"#,
    )
    .expect("config should be written");

    let config = PunchConfig::load(&config_path).expect("config should load");
    assert_eq!(config.tunnels.len(), 2);
    assert_eq!(config.tunnels[0].domain, "api.example.com");
    assert_eq!(config.tunnels[0].https, Some(true));
}
