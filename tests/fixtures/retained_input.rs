use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn mark(root: &Path, name: &str, value: u32) -> Result<()> {
    let staged = root.join(format!("{name}.staged"));
    fs::write(&staged, value.to_string())?;
    fs::rename(staged, root.join(name))?;
    Ok(())
}

fn wait(root: &Path, name: &str) -> Result<()> {
    let started = Instant::now();
    while !root.join(name).exists() {
        if started.elapsed() > Duration::from_secs(30) {
            return Err("Retained input fixture barrier was not released".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn input() -> Result<Vec<u8>> {
    let mut body = Vec::new();
    io::stdin().take(256 * 1024 + 1).read_to_end(&mut body)?;
    if body.len() > 256 * 1024 {
        return Err("Retained fixture input exceeds its bound".into());
    }
    Ok(body)
}

fn descendant(root: &Path, pipes: bool) -> Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("descendant")
        .arg(root)
        .stdin(if pipes {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(if pipes {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stderr(if pipes {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .spawn()?;
    mark(root, "descendant", child.id())?;
    wait(root, "descendant-ready")?;
    if root.join("wait-child").exists() {
        child.wait()?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("Retained fixture mode is missing")?;
    let root = PathBuf::from(args.next().ok_or("Retained fixture root is missing")?);
    match mode.as_str() {
        "binary" => {
            io::stdout().write_all(&input()?)?;
            io::stderr().write_all(b"eof")?;
        }
        "output-first" => {
            io::stdout().write_all(&vec![b'O'; 128 * 1024])?;
            io::stdout().flush()?;
            io::stderr().write_all(&vec![b'E'; 128 * 1024])?;
            io::stderr().flush()?;
            io::stdout().write_all(&input()?)?;
        }
        "hold" | "overflow" | "early" => {
            descendant(&root, true)?;
            mark(&root, "ready", std::process::id())?;
            wait(&root, "release")?;
            if mode == "overflow" {
                io::stdout().write_all(&vec![b'X'; 128 * 1024])?;
                io::stdout().flush()?;
                wait(&root, "finish")?;
            } else if mode == "hold" {
                wait(&root, "finish")?;
            }
        }
        "consume-descendant" => {
            let body = input()?;
            descendant(&root, false)?;
            io::stdout().write_all(&body)?;
            io::stdout().flush()?;
            mark(&root, "ready", std::process::id())?;
        }
        "late" => {
            let body = input()?;
            mark(&root, "ready", std::process::id())?;
            wait(&root, "release")?;
            io::stdout().write_all(&body)?;
        }
        "descendant" => {
            mark(&root, "descendant-ready", std::process::id())?;
            wait(&root, "finish-child")?;
        }
        "exit" => {}
        _ => return Err("Unknown retained input fixture mode".into()),
    }
    Ok(())
}
