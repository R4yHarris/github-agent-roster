use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(env::var_os("GH_CONFIG_DIR").ok_or("Missing owned GH scope")?);
    let mode = fs::read_to_string(directory.join("mode"))?;
    let args = env::args().skip(1).collect::<Vec<_>>();
    let method = args
        .windows(2)
        .find(|pair| pair.first().is_some_and(|value| value == "--method"))
        .and_then(|pair| pair.get(1))
        .ok_or("Missing API method")?;
    let endpoint = args
        .iter()
        .find(|value| value.as_str() == "user" || value.starts_with("repos/"))
        .ok_or("Missing API endpoint")?;
    let mut calls = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("calls"))?;
    writeln!(calls, "{method} {endpoint}")?;
    calls.sync_all()?;
    if env::var("GH_TOKEN").ok().as_deref() != Some("test-only-explicit-human-token")
        || env::var_os("GITHUB_TOKEN").is_some()
    {
        return Err("The selected human credential was not pinned".into());
    }
    if endpoint == "user" {
        match mode.trim() {
            "bot" => println!(r#"{{"id":7001,"type":"Bot","login":"owned-bot"}}"#),
            "bad-user" => println!(r#"{{"id":0,"type":"User","login":"owned-human"}}"#),
            "read-fail" => {
                eprintln!("test-only-explicit-human-token must not escape");
                std::process::exit(3);
            }
            _ => println!(r#"{{"id":7001,"type":"User","login":"owned-human"}}"#),
        }
        return Ok(());
    }
    if endpoint.contains("/commits/") && endpoint.ends_with("/pulls?per_page=100") {
        match mode.trim() {
            "no-pr" => println!("[]"),
            "foreign" => println!(
                r#"[{{"number":42,"base":{{"repo":{{"full_name":"foreign/repository"}}}}}}]"#
            ),
            "ambiguous" => println!(
                r#"[{{"number":42,"base":{{"repo":{{"full_name":"owned/analytics"}}}}}},{{"number":43,"base":{{"repo":{{"full_name":"owned/analytics"}}}}}}]"#
            ),
            "malformed" => println!("not-json test-only-explicit-human-token"),
            "drift" => {
                let path = env::current_dir()?.join(".git").join("config");
                let config = fs::read_to_string(&path)?;
                fs::write(
                    path,
                    config.replace(
                        "https://github.com/owned/analytics.git",
                        "https://github.com/owned/different.git",
                    ),
                )?;
                println!(
                    r#"[{{"number":42,"base":{{"repo":{{"full_name":"owned/analytics"}}}}}}]"#
                );
            }
            _ => {
                println!(r#"[{{"number":42,"base":{{"repo":{{"full_name":"owned/analytics"}}}}}}]"#)
            }
        }
        return Ok(());
    }
    if endpoint == "repos/owned/analytics/issues/42/comments" && method == "POST" {
        let mut source = String::new();
        std::io::stdin().take(8193).read_to_string(&mut source)?;
        if source.len() > 8192 {
            return Err("Owned human body exceeded its bound".into());
        }
        fs::write(directory.join("posted.json"), &source)?;
        match mode.trim() {
            "post-fail" => {
                eprintln!("test-only-explicit-human-token must not escape");
                std::process::exit(4);
            }
            "post-hold" => {
                fs::write(directory.join("ready"), std::process::id().to_string())?;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            "bad-ack" => println!(r#"{{"id":9001,"body":"not the actual human body"}}"#),
            _ => {
                let body = source
                    .strip_prefix("{\"body\":")
                    .and_then(|value| value.strip_suffix('}'))
                    .ok_or("Owned human body must be one compact JSON field")?;
                println!("{{\"id\":9001,\"body\":{body}}}");
            }
        }
        return Ok(());
    }
    Err("Unexpected owned GitHub operation".into())
}
