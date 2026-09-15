//! `ald new` — scaffold a new resource from a template.
//!
//! Writes a real, minimal resource directory with a valid manifest. Not a
//! stub generator: the output passes `ald resource validate`.

use std::fs;
use std::path::Path;
use std::process::exit;

use ald_resource::Manifest;

pub fn run(args: &[String]) {
    if args.len() < 2 {
        eprintln!("usage: ald new <resource-name> <path> [--lua|--js|--ts]");
        exit(1);
    }
    let name = &args[0];
    let path = &args[1];
    let lang = match args.get(2).map(|s| s.as_str()) {
        Some("--js") => Lang::Js,
        Some("--ts") => Lang::Ts,
        // Lua is the default.
        _ => Lang::Lua,
    };

    if !is_valid_resource_name(name) {
        eprintln!("invalid resource name '{name}': lowercase, digits, hyphens only; must start with a letter");
        exit(1);
    }

    let root = Path::new(path);
    if let Err(e) = fs::create_dir_all(root) {
        eprintln!("cannot create {path}: {e}");
        exit(1);
    }
    for dir in ["server", "client", "shared", "ui"] {
        if let Err(e) = fs::create_dir_all(root.join(dir)) {
            eprintln!("cannot create {dir}: {e}");
            exit(1);
        }
    }

    // Manifest is written from the parsed+validated struct, so a freshly
    // scaffolded resource always validates.
    let manifest = Manifest {
        name: name.clone(),
        version: "0.1.0".to_string(),
        author: Some("unknown".to_string()),
        description: Some(format!("{name} resource")),
        license: None,
        minimum_runtime: None,
        dependencies: Vec::new(),
        optional_dependencies: Vec::new(),
        client_scripts: vec![lang.client_entry()],
        server_scripts: vec![lang.server_entry()],
        shared_scripts: vec![],
        ui: vec![],
        assets: Vec::new(),
        capabilities: Vec::new(),
        exports: Vec::new(),
        imports: Vec::new(),
        framework: None,
        compatibility: Vec::new(),
        configuration: Vec::new(),
        database_migrations: Vec::new(),
    };
    let toml_text = toml::to_string(&manifest).expect("scaffolded manifest serializes");
    match Manifest::parse(&toml_text) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("internal error: scaffolded manifest failed validation: {e}");
            exit(1);
        }
    }
    if let Err(e) = fs::write(root.join("ald_manifest.toml"), &toml_text) {
        eprintln!("cannot write manifest: {e}");
        exit(1);
    }

    if let Err(e) = fs::write(root.join(lang.server_entry()), lang.server_body(name)) {
        eprintln!("cannot write server script: {e}");
        exit(1);
    }
    if let Err(e) = fs::write(root.join(lang.client_entry()), lang.client_body(name)) {
        eprintln!("cannot write client script: {e}");
        exit(1);
    }

    println!("created {name} ({lang}) at {path}");
    println!("next: ald resource validate {}/ald_manifest.toml", path);
}

#[derive(Debug, Clone, Copy)]
enum Lang {
    Lua,
    Js,
    Ts,
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Lang::Lua => "lua",
            Lang::Js => "javascript",
            Lang::Ts => "typescript",
        })
    }
}

impl Lang {
    fn server_entry(self) -> String {
        match self {
            Lang::Lua => "server/init.lua".into(),
            Lang::Js => "server/init.js".into(),
            Lang::Ts => "server/init.ts".into(),
        }
    }
    fn client_entry(self) -> String {
        match self {
            Lang::Lua => "client/init.lua".into(),
            Lang::Js => "client/init.js".into(),
            Lang::Ts => "client/init.ts".into(),
        }
    }
    fn server_body(self, name: &str) -> String {
        match self {
            Lang::Lua => format!(
                "-- {name} server entry\n-- Runs in the sandboxed Lua runtime. Capabilities come from ald_manifest.toml.\n\nAldivine.Events.on('{name}:server:start', function(payload)\n    print(('{name} server started'):format())\nend)\n"
            ),
            Lang::Js => format!(
                "// {name} server entry\n// Runs in the sandboxed QuickJS runtime.\n\nAldivine.Events.on('{name}:server:start', () => {{\n    console.log('{name} server started');\n}});\n"
            ),
            Lang::Ts => format!(
                "// {name} server entry\n// TypeScript is compiled to JavaScript before execution.\n\nAldivine.Events.on('{name}:server:start', () => {{\n    console.log('{name} server started');\n}});\n"
            ),
        }
    }
    fn client_body(self, name: &str) -> String {
        match self {
            Lang::Lua => format!("-- {name} client entry\n\nAldivine.Events.on('{name}:client:start', function(payload)\n    print(('{name} client started'):format())\nend)\n"),
            Lang::Js => format!("// {name} client entry\n\nAldivine.Events.on('{name}:client:start', () => {{\n    console.log('{name} client started');\n}});\n"),
            Lang::Ts => format!("// {name} client entry\n\nAldivine.Events.on('{name}:client:start', () => {{\n    console.log('{name} client started');\n}});\n"),
        }
    }
}

/// Resource names must be DNS-like so they are safe as directory names and as
/// event namespaces.
fn is_valid_resource_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && name.len() <= 64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(is_valid_resource_name("my-job"));
        assert!(is_valid_resource_name("a"));
        assert!(!is_valid_resource_name(""));
        assert!(!is_valid_resource_name("MyJob"));
        assert!(!is_valid_resource_name("1job"));
        assert!(!is_valid_resource_name("job with space"));
        assert!(!is_valid_resource_name("job_other"));
    }
}
