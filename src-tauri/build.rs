fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        println!("cargo:rerun-if-env-changed=BPSR_SKIP_MANIFEST");

        let skip_manifest = std::env::var("BPSR_SKIP_MANIFEST").is_ok_and(|v| v == "1");
        if !skip_manifest {
            use embed_manifest::manifest::ExecutionLevel;
            use embed_manifest::{embed_manifest, new_manifest};

            embed_manifest(
                new_manifest("BpsrModuleOptimizer")
                    .requested_execution_level(ExecutionLevel::RequireAdministrator),
            )
            .expect("embed manifest failed");
        }
    }

    tauri_build::build()
}
