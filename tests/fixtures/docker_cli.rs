use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn phase(state: &Path, operation: &str, phase: &str) -> Result<()> {
    let path = state.join("events");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let at = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_micros(),
    )?;
    let line = format!(
        "{{\"at_us\":{at},\"pid\":{},\"operation\":{operation:?},\"phase\":{phase:?}}}\n",
        std::process::id()
    );
    if file
        .metadata()?
        .len()
        .checked_add(u64::try_from(line.len())?)
        .is_none_or(|size| size > 65_536)
    {
        return Err("owned fixture diagnostic bound exceeded".into());
    }
    file.write_all(line.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn required(values: &[String], key: &str) -> Result<String> {
    values
        .windows(2)
        .find(|pair| pair.first().is_some_and(|value| value == key))
        .and_then(|pair| pair.get(1))
        .cloned()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("missing {key}")).into())
}

fn repeated(values: &[String], key: &str) -> Vec<String> {
    values
        .windows(2)
        .filter(|pair| pair.first().is_some_and(|value| value == key))
        .filter_map(|pair| pair.get(1).cloned())
        .collect()
}

fn array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn create_barrier(state: &Path) -> Result<()> {
    let staged = state.join("create-ready-staged");
    fs::write(&staged, std::process::id().to_string())?;
    fs::rename(staged, state.join("create-ready"))?;
    let started = std::time::Instant::now();
    while !state.join("release-create").exists() {
        if started.elapsed() > Duration::from_secs(30) {
            return Err("owned native create barrier was not released".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}

fn find(state: &Path, selector: &str) -> Result<Option<(String, Vec<String>)>> {
    let mut entries = fs::read_dir(state)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|name| name.to_str()) != Some("args") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or("invalid fixture ID")?
            .to_owned();
        let values = fs::read_to_string(&path)?
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if id == selector || required(&values, "--name")? == selector {
            return Ok(Some((id, values)));
        }
    }
    Ok(None)
}

fn emit_inspection(state: &Path, id: &str, values: &[String], mode: &str) -> Result<()> {
    let name = required(values, "--name")?;
    let image_position = values
        .iter()
        .position(|value| value.starts_with("sha256:"))
        .ok_or("image missing")?;
    let image = values.get(image_position).ok_or("image missing")?;
    let args = values.get(image_position + 1..).ok_or("command missing")?;
    let mut labels = Vec::new();
    for label in repeated(values, "--label") {
        let (key, value) = label.split_once('=').ok_or("label missing")?;
        let value = if mode == "wrong-owner" && key == "io.github-agent-roster.owner" {
            "foreign-owner"
        } else {
            value
        };
        labels.push(format!("{key:?}:{value:?}"));
    }
    let env = repeated(values, "--env");
    let mut mounts = Vec::new();
    for mount in repeated(values, "--mount") {
        let parts = mount.split(',').collect::<Vec<_>>();
        let source = parts
            .iter()
            .find_map(|value| value.strip_prefix("source="))
            .ok_or("mount source missing")?;
        let destination = parts
            .iter()
            .find_map(|value| value.strip_prefix("target="))
            .ok_or("mount target missing")?;
        mounts.push(format!(
            "{{\"Type\":\"bind\",\"Source\":{source:?},\"Destination\":{destination:?},\"RW\":{}}}",
            mode == "writable-mount"
        ));
    }
    let entrypoint = required(values, "--entrypoint")?;
    let user = required(values, "--user")?;
    let workdir = required(values, "--workdir")?;
    let network = required(values, "--network")?;
    let cap = required(values, "--cap-drop")?;
    let security = required(values, "--security-opt")?;
    let log = required(values, "--log-driver")?;
    let ipc = required(values, "--ipc")?;
    let cgroup = required(values, "--cgroupns")?;
    let restart = required(values, "--restart")?;
    let pid = if mode == "wrong-namespace" {
        "host"
    } else {
        ""
    };
    let tmpfs = required(values, "--tmpfs")?;
    let tmpfs = tmpfs
        .strip_prefix("/tmp:")
        .ok_or("tmpfs location missing")?;
    let pids = required(values, "--pids-limit")?.parse::<u64>()?;
    let memory = if mode == "wrong-isolation" {
        1_u64
    } else {
        268_435_456_u64
    };
    let nano = if required(values, "--cpus")? == "1" {
        1_000_000_000_u64
    } else {
        0
    };
    let started = state.join(format!("{id}.started")).exists();
    let running = state.join(format!("{id}.running")).exists();
    let status = if running {
        "running"
    } else if started {
        "exited"
    } else {
        "created"
    };
    let exit = if mode == "exit-one" { 1 } else { 0 };
    let readonly = values.iter().any(|value| value == "--read-only");
    let command = if mode == "wrong-command" {
        array(&["unapproved".to_owned()])
    } else {
        array(args)
    };
    println!(
        "{{\"Id\":{id:?},\"Name\":{name:?},\"Image\":{image:?},\"Config\":{{\"Image\":{image:?},\"User\":{user:?},\"WorkingDir\":{workdir:?},\"Entrypoint\":[{entrypoint:?}],\"Cmd\":{command},\"Env\":{},\"Labels\":{{{}}},\"Tty\":false,\"OpenStdin\":false}},\"HostConfig\":{{\"NetworkMode\":{network:?},\"ReadonlyRootfs\":{readonly},\"Privileged\":false,\"AutoRemove\":false,\"CapDrop\":[{cap:?}],\"CapAdd\":null,\"SecurityOpt\":[{security:?}],\"Memory\":{memory},\"MemorySwap\":268435456,\"NanoCpus\":{nano},\"PidsLimit\":{pids},\"IpcMode\":{ipc:?},\"PidMode\":{pid:?},\"UTSMode\":\"\",\"UsernsMode\":\"\",\"CgroupnsMode\":{cgroup:?},\"RestartPolicy\":{{\"Name\":{restart:?},\"MaximumRetryCount\":0}},\"LogConfig\":{{\"Type\":{log:?}}},\"Devices\":[],\"DeviceRequests\":null,\"VolumesFrom\":null,\"Binds\":null,\"Tmpfs\":{{\"/tmp\":{tmpfs:?}}},\"Ulimits\":[{{\"Name\":\"nofile\",\"Soft\":256,\"Hard\":256}}]}},\"Mounts\":[{}],\"State\":{{\"Running\":{running},\"Status\":{status:?},\"ExitCode\":{exit},\"OOMKilled\":false,\"Dead\":false,\"Error\":\"\"}}}}",
        array(&env),
        labels.join(","),
        mounts.join(","),
        name = format!("/{name}"),
    );
    Ok(())
}

fn state_root() -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let root = executable
        .parent()
        .ok_or("fixture executable parent missing")?
        .join("docker-state");
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn main() -> Result<()> {
    let values = std::env::args().skip(1).collect::<Vec<_>>();
    let state = state_root()?;
    let mode = read(&state.join("mode"))?.ok_or("owned fixture mode is missing")?;
    let mut log = OpenOptions::new()
        .append(true)
        .create(true)
        .open(state.join("calls"))?;
    writeln!(log, "{}", array(&values))?;
    log.sync_all()?;
    for key in [
        "GITHUB_TOKEN",
        "GITHUB_APP_ID",
        "ROSTER_API_KEY",
        "NODE_OPTIONS",
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
    ] {
        if std::env::var_os(key).is_some() {
            fail("credential or ambient Docker environment leaked");
        }
    }
    if values.first().map(String::as_str) != Some("--config")
        || values.get(2).map(String::as_str) != Some("--host")
        || values.get(3).map(String::as_str) != Some("npipe:////./pipe/docker_engine")
    {
        fail("explicit isolated config and local endpoint missing");
    }
    let command = values.get(4..).ok_or("command missing")?;
    let first = command.first().map_or("", String::as_str);
    let second = command.get(1).map_or("", String::as_str);
    let operation = match (first, second) {
        ("image", "inspect") => "image-inspect",
        ("container", "create") => "create",
        ("container", "inspect") => "inspect",
        ("container", "start") => "start",
        ("container", "rm") => "rm",
        _ => "invalid",
    };
    phase(&state, operation, "received")?;
    if (first, second) == ("image", "inspect") {
        if mode == "daemon-missing" {
            fail("Cannot connect to owned fake Docker daemon");
        }
        if mode == "image-missing" {
            fail("Error: No such image: owned fixture");
        }
        if mode == "bad-json" {
            println!("not json");
            return Ok(());
        }
        let image = command.last().ok_or("image missing")?;
        let image = if mode == "wrong-image" {
            format!("sha256:{}", "b".repeat(64))
        } else {
            image.clone()
        };
        let env = if mode == "unsafe-image-env" {
            "[\"GITHUB_TOKEN=owned-fixture\"]"
        } else {
            "[]"
        };
        phase(&state, operation, "reply")?;
        println!(
            "{{\"Id\":{image:?},\"Os\":\"linux\",\"Config\":{{\"Env\":{env},\"Volumes\":null}}}}"
        );
        return Ok(());
    }
    if first != "container" {
        fail("fixture refuses non-container mutation commands");
    }
    match second {
        "create" => {
            if mode == "create-ambiguous" {
                println!("unknown create outcome");
                return Ok(());
            }
            for (key, expected) in [
                ("--pull", "never"),
                ("--network", "none"),
                ("--cap-drop", "ALL"),
                ("--security-opt", "no-new-privileges"),
                ("--user", "65534:65534"),
                ("--memory", "256m"),
                ("--memory-swap", "256m"),
                ("--cpus", "1"),
                ("--pids-limit", "32"),
                ("--ulimit", "nofile=256:256"),
                ("--ipc", "none"),
                ("--cgroupns", "private"),
                ("--restart", "no"),
                ("--log-driver", "none"),
            ] {
                if required(command, key)? != expected {
                    fail("fixture rejected a changed isolation argument");
                }
            }
            if !command.iter().any(|value| value == "--read-only") {
                fail("readonly root missing");
            }
            if mode == "create-before-object" {
                create_barrier(&state)?;
            }
            let counter = read(&state.join("counter"))?
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("owned fixture counter overflow")?;
            fs::write(state.join("counter"), counter.to_string())?;
            let id = format!("{counter:064x}");
            fs::write(state.join(format!("{id}.args")), command.join("\n"))?;
            phase(&state, operation, "object-recorded")?;
            if mode == "create-after-object" {
                create_barrier(&state)?;
            }
            if mode != "create-no-cid" {
                let reported = if mode == "create-wrong-cid" {
                    "e".repeat(64)
                } else {
                    id.clone()
                };
                fs::write(required(command, "--cidfile")?, &reported)?;
                phase(&state, operation, "cid-written")?;
                println!("{reported}");
            }
        }
        "inspect" => {
            let selector = command.last().ok_or("inspect selector missing")?;
            match find(&state, selector)? {
                Some((id, values)) => {
                    phase(&state, operation, "owned-object-reply")?;
                    emit_inspection(&state, &id, &values, &mode)?;
                }
                None if mode == "absence-unknown" => {
                    fail("Cannot connect to owned fake Docker daemon")
                }
                None => {
                    phase(&state, operation, "absent-reply")?;
                    fail(&format!("Error: No such object: {selector}"));
                }
            }
        }
        "start" => {
            let selector = command.last().ok_or("start selector missing")?;
            let (id, values) = find(&state, selector)?.ok_or("owned container missing")?;
            fs::write(state.join(format!("{id}.started")), b"started")?;
            let position = values
                .iter()
                .position(|value| value.starts_with("sha256:"))
                .ok_or("image missing")?;
            let args = values
                .get(position + 1..)
                .ok_or("container arguments missing")?;
            if ["timeout", "cancel", "overflow"].contains(&mode.as_str()) {
                fs::write(state.join(format!("{id}.running")), b"running")?;
                fs::write(state.join("ready"), b"ready")?;
                phase(&state, operation, "running-ready")?;
                if mode == "overflow" {
                    io::stdout().write_all(&vec![b'x'; 131_072])?;
                    io::stdout().flush()?;
                }
                std::thread::sleep(Duration::from_secs(60));
            }
            if mode == "product-mutation" {
                fs::write("src/app.mjs", b"export function answer() { return 999; }\n")?;
            }
            if mode == "task-mutation" {
                fs::write(
                    "TASK.md",
                    b"Changed task must remain untouched by cleanup\n",
                )?;
            }
            if mode == "invalid-output" {
                io::stdout().write_all(&[255])?;
                return Ok(());
            }
            if args
                .first()
                .is_some_and(|value| value == "--input-type=commonjs")
            {
                println!(
                    "{}",
                    if mode == "node-version" {
                        "v18.0.0"
                    } else {
                        "v20.19.5"
                    }
                );
            } else if args.first().is_some_and(|value| value == "read-product") {
                let path = args.get(1).ok_or("product operand missing")?;
                let mount = repeated(&values, "--mount")
                    .into_iter()
                    .find(|value| value.contains("target=/product"))
                    .ok_or("product mount missing")?;
                let source = mount
                    .split(',')
                    .find_map(|value| value.strip_prefix("source="))
                    .ok_or("product source missing")?;
                print!("{}", fs::read_to_string(Path::new(source).join(path))?);
            } else {
                println!(
                    "{}",
                    if mode == "mismatch" {
                        "wrong"
                    } else {
                        "verified"
                    }
                );
            }
        }
        "rm" => {
            if mode == "cleanup-fail" {
                fail("owned fixture removal failed");
            }
            let selector = command.last().ok_or("rm selector missing")?;
            if selector.len() != 64 || !selector.bytes().all(|value| value.is_ascii_hexdigit()) {
                fail("fixture refuses removal by a name instead of a full ID");
            }
            if let Some((id, _)) = find(&state, selector)? {
                fs::remove_file(state.join(format!("{id}.args")))?;
                for suffix in ["started", "running"] {
                    match fs::remove_file(state.join(format!("{id}.{suffix}"))) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            println!("{selector}");
            phase(&state, operation, "removed-reply")?;
        }
        _ => fail("unsupported owned fixture operation"),
    }
    phase(&state, operation, "complete")?;
    Ok(())
}
