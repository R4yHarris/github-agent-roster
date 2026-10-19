use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        let output = PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cargo build output is unavailable",
            )
        })?);
        fs::write(output.join("libktmw32.a"), b"INPUT(-lwinapi_ktmw32)\n")?;
        println!("cargo:rustc-link-search=native={}", output.display());
    }
    Ok(())
}
