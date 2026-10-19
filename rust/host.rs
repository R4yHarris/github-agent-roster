use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::Error;
use crate::config;
use crate::execution::{backend, tools};
use crate::json::Value;
use crate::paths::{self, FileIdentity};
use crate::state::secure;

#[derive(Debug, Clone)]
pub struct Image {
    path: PathBuf,
    identity: FileIdentity,
    size: u64,
    modified: SystemTime,
}

impl Image {
    pub fn approve(path: &Path) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(Error::type_error(
                "Host executable approval requires an absolute installed image, not a PATH name",
            ));
        }
        let selected = paths::file_information(path)?;
        let path = paths::strip_verbatim(path.to_path_buf());
        secure::check_components(&path)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| secure::io_error("Inspect approved executable", &path, error))?;
        if !metadata.is_file()
            || secure::linked(&metadata)
            || selected.identity != paths::file_information(&path)?.identity
            || cfg!(windows)
                && !path.extension().is_some_and(|extension| {
                    extension.to_string_lossy().eq_ignore_ascii_case("exe")
                })
        {
            return Err(Error::type_error(
                "Approved executable must be a real installed binary, not a link, shell or script",
            ));
        }
        let modified = metadata.modified().map_err(|error| {
            secure::io_error("Read approved executable modification time", &path, error)
        })?;
        Ok(Self {
            path,
            identity: selected.identity,
            size: metadata.len(),
            modified,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn require_current(&self) -> Result<(), Error> {
        secure::check_components(&self.path)?;
        let metadata = fs::symlink_metadata(&self.path)
            .map_err(|error| secure::io_error("Recheck approved executable", &self.path, error))?;
        if !metadata.is_file()
            || secure::linked(&metadata)
            || paths::file_information(&self.path)?.identity != self.identity
            || metadata.len() != self.size
            || metadata.modified().map_err(|error| {
                secure::io_error("Recheck executable modification time", &self.path, error)
            })? != self.modified
        {
            return Err(Error::new(
                "HostImageChanged",
                "An approved host executable changed; the invocation is revoked",
            )
            .coded("ROSTER_HOST_IMAGE_CHANGED"));
        }
        Ok(())
    }
}

pub fn selected_path(environment: &Value, name: &str) -> Result<Option<PathBuf>, Error> {
    config::environment_value(environment, name)?
        .map(|value| {
            let path = PathBuf::from(value.to_utf8()?);
            if !path.is_absolute() {
                return Err(Error::type_error(format!(
                    "{name} must name an absolute installed path"
                )));
            }
            Ok(path)
        })
        .transpose()
}

pub fn minimal_environment(environment: &Value) -> Result<BTreeMap<String, String>, Error> {
    let mut result = BTreeMap::new();
    let allowed = [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "TEMP",
        "TMP",
        "LANG",
        "LC_ALL",
        "TZ",
    ];
    let entries = crate::domain::members(environment)
        .ok_or_else(|| Error::type_error("Host environment must be an object"))?;
    for (name, value) in entries {
        let name = name.to_utf8()?;
        if allowed.iter().any(|allowed| {
            if cfg!(windows) {
                name.eq_ignore_ascii_case(allowed)
            } else {
                name == *allowed
            }
        }) {
            let value = value
                .as_string()
                .ok_or_else(|| Error::type_error("Host environment values must be text"))?
                .to_utf8()?;
            if value.contains('\0') {
                return Err(Error::type_error("Host environment must not contain NUL"));
            }
            let name = if cfg!(windows) {
                name.to_ascii_uppercase()
            } else {
                name
            };
            if result.insert(name, value).is_some() {
                return Err(Error::type_error(
                    "Host environment contains ambiguous selected aliases",
                ));
            }
        }
    }
    Ok(result)
}

pub fn contracts_override(environment: &Value) -> Result<Option<PathBuf>, Error> {
    config::environment_value(environment, "GITHUB_AGENT_CONTRACTS")?
        .map(|value| {
            let value = value.to_utf8()?;
            if value.contains(['\0', '\r', '\n']) {
                return Err(Error::type_error(
                    "GITHUB_AGENT_CONTRACTS must contain one nonempty path",
                ));
            }
            Ok(PathBuf::from(value))
        })
        .transpose()
}

pub fn invocation_host(
    environment: &Value,
    native_cli: &Path,
) -> Result<(tools::Host, backend::Approval), Error> {
    let git = selected_path(environment, "ROSTER_GIT")?.ok_or_else(|| {
        crate::execution::unsupported(
            "Local delivery requires ROSTER_GIT pointing to an explicitly approved Git image",
        )
    })?;
    let git = Image::approve(&git)?;
    let node = selected_path(environment, "ROSTER_NODE20")?
        .map(|path| Image::approve(&path))
        .transpose()?;
    let cargo = selected_path(environment, "ROSTER_CARGO")?
        .map(|path| Image::approve(&path))
        .transpose()?;
    let native_cli = Image::approve(native_cli)?;
    let mut cargo_environment = BTreeMap::new();
    for name in [
        "CARGO_HOME",
        "RUSTUP_HOME",
        "CARGO_TARGET_DIR",
        "RUSTFLAGS",
        "RUSTUP_TOOLCHAIN",
    ] {
        if let Some(value) = config::environment_value(environment, name)? {
            cargo_environment.insert(name.to_owned(), value.to_utf8()?);
        }
    }
    let host = tools::Host {
        git: git.path().to_path_buf(),
        node20: node.as_ref().map(|image| image.path().to_path_buf()),
        cargo: cargo.map(|image| tools::CargoApproval {
            executable: image.path().to_path_buf(),
            environment: cargo_environment,
        }),
        contracts: contracts_override(environment)?,
        environment: minimal_environment(environment)?,
    };
    let approval = backend::Approval {
        counterfactual: true,
        classifications: true,
        hooks_node: host.node20.clone(),
        native_cli: Some(native_cli.path().to_path_buf()),
        node: host.node20.clone(),
        submodules: local_submodules(environment)?,
        timeout_ms: config::environment_value(environment, "ROSTER_BACKEND_TIMEOUT_MS")?
            .map(|value| {
                value
                    .to_utf8()?
                    .parse::<u64>()
                    .ok()
                    .filter(|value| {
                        (1..=crate::execution::policy::FULL_TEST_TIMEOUT_MS).contains(value)
                    })
                    .ok_or_else(|| {
                        Error::type_error(
                            "ROSTER_BACKEND_TIMEOUT_MS must be an integer between 1 and 900000",
                        )
                    })
            })
            .transpose()?,
        ..backend::Approval::default()
    };
    Ok((host, approval))
}

fn local_submodules(environment: &Value) -> Result<Vec<backend::LocalSubmodule>, Error> {
    let Some(source) = config::environment_value(environment, "ROSTER_LOCAL_SUBMODULES")? else {
        return Ok(Vec::new());
    };
    if source.utf8_len() > 8192 {
        return Err(Error::type_error("ROSTER_LOCAL_SUBMODULES exceeds 8 KiB"));
    }
    let value = crate::json::parse(source).map_err(|_| Error::type_error(
        "ROSTER_LOCAL_SUBMODULES must be JSON [{\"path\":\"relative/gitlink\",\"source\":\"absolute local source\"}]",
    ))?;
    let values = crate::domain::items(&value)
        .filter(|values| values.len() <= 16)
        .ok_or_else(|| {
            Error::type_error(
                "ROSTER_LOCAL_SUBMODULES must contain at most 16 explicit local sources",
            )
        })?;
    let mut modules = Vec::new();
    for value in values {
        let fields = crate::domain::members(value)
            .filter(|fields| fields.len() == 2)
            .ok_or_else(|| {
                Error::type_error("Local submodule approval needs exactly path and source")
            })?;
        if !fields.iter().all(|(key, _)| {
            key == &crate::text::Text::from("path") || key == &crate::text::Text::from("source")
        }) {
            return Err(Error::type_error(
                "Local submodule approval supports only path and source",
            ));
        }
        let path = value
            .get("path")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Local submodule path must be text"))?
            .to_utf8()?;
        let source = value
            .get("source")
            .and_then(Value::as_string)
            .ok_or_else(|| Error::type_error("Local submodule source must be text"))?
            .to_utf8()?;
        if source.contains(['\0', '\r', '\n']) {
            return Err(Error::type_error(
                "Local submodule source must be one absolute path",
            ));
        }
        modules.push(backend::LocalSubmodule {
            path,
            source: PathBuf::from(source),
        });
    }
    backend::Approval {
        submodules: modules.clone(),
        ..backend::Approval::default()
    }
    .validate()?;
    Ok(modules)
}

pub(crate) fn pins(host: &tools::Host) -> Result<Vec<Image>, Error> {
    [
        Some(host.git.as_path()),
        host.node20.as_deref(),
        host.cargo.as_ref().map(|cargo| cargo.executable.as_path()),
    ]
    .into_iter()
    .flatten()
    .filter(|path| path.is_absolute())
    .map(Image::approve)
    .collect()
}
