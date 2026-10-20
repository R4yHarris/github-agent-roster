use std::fs;
use std::path::Path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn rust_skills_are_complete_discoverable_and_native_first() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in ["rust-implementation", "rust-cargo", "rust-tokio"] {
        let source = fs::read_to_string(root.join("skills").join(name).join("SKILL.md"))?;
        assert!(source.starts_with("---\n") || source.starts_with("---\r\n"));
        assert!(source.contains(&format!("name: {name}")));
        assert!(source.contains("description: \"Use when "));
        for heading in ["When to use", "Procedure", "Stop condition"] {
            assert!(source.contains(&format!("## {heading}")));
        }
        assert!(source.lines().count() <= 40);
        assert!(!source.contains("npm run test:rust"));
        assert!(!source.contains("Keep Node operational"));
        if name == "rust-tokio" {
            assert!(source.contains("ROSTER_PROCESS_CLEANUP"));
            assert!(source.contains("metadata/origin Git"));
            assert!(source.contains("not OS-failure proof"));
        }
        let discovery = fs::read_to_string(
            root.join(".github")
                .join("skills")
                .join(name)
                .join("SKILL.md"),
        )?;
        assert!(discovery.contains(&format!("name: {name}")));
        assert!(discovery.contains(&format!("../../../skills/{name}/SKILL.md")));
    }
    Ok(())
}

#[test]
fn rust_and_cargo_instructions_have_scoped_discovery_and_no_node_preservation_rule() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, glob) in [
        ("rust", "rust/**/*.rs"),
        (
            "cargo",
            "Cargo.toml,Cargo.lock,rust/**/Cargo.toml,**/.cargo/config.toml",
        ),
    ] {
        let source = fs::read_to_string(
            root.join(".github")
                .join("instructions")
                .join(format!("{name}.instructions.md")),
        )?;
        assert!(source.contains("description: \"Use when "));
        assert!(source.contains(&format!("applyTo: \"{glob}\"")));
        assert!(!source.contains("npm run test:rust"));
        assert!(!source.contains("Preserve Node 20 ESM"));
    }
    assert!(!root.join("package.json").exists());
    assert!(!root.join("src").join("cli.mjs").exists());
    assert!(!root.join("scripts").join("legacy-oracle.mjs").exists());
    assert!(
        !root
            .join("scripts")
            .join("check-roster-scope.mjs")
            .exists()
    );
    assert!(
        !root
            .join("scripts")
            .join("check-agent-trailers.mjs")
            .exists()
    );
    assert!(
        !root
            .join("scripts")
            .join("check-pr-agent-trailers.mjs")
            .exists()
    );
    Ok(())
}
