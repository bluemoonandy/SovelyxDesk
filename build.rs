fn main() {
    let brand = build_brand();
    build_metadata();
    build_gpu();
    println!("cargo:rerun-if-changed=assets/brand");
    println!("cargo:rerun-if-changed=translations");
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("cupertino".into())
            .with_bundled_translations("translations"),
    )
    .expect("Slint interface compilation failed");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/brand/sovelyx-desk.ico")
            .set("ProductName", brand["app_name"].as_str().unwrap())
            .set("FileDescription", brand["description"].as_str().unwrap())
            .set("CompanyName", brand["publisher"].as_str().unwrap())
            .set("OriginalFilename", brand["executable"].as_str().unwrap())
            .set("InternalName", brand["app_name"].as_str().unwrap())
            .set("LegalCopyright", "MacCanvas upstream authors and Sovelyx contributors. See --licenses.")
            .set_manifest(
                r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
<requestedExecutionLevel level="asInvoker" uiAccess="false"/>
</requestedPrivileges></security></trustInfo>
<application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings>
<dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
<longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
</windowsSettings></application>
</assembly>"#,
            )
            .compile()
            .expect("Windows resource compilation failed");
    }

    fn build_gpu() {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        println!("cargo:rerun-if-changed=native/gpu_hevc.cpp");
        println!("cargo:rerun-if-changed=native/nvcodec");
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .static_crt(true)
            .opt_level(2)
            .warnings(false)
            .define("NOMINMAX", "1")
            .define("WIN32_LEAN_AND_MEAN", "1")
            .flag("/EHsc")
            .include("native/nvcodec")
            .file("native/gpu_hevc.cpp")
            .compile("maccanvas_gpu");
        for library in ["d3d11", "dxgi", "d3dcompiler"] {
            println!("cargo:rustc-link-lib={library}");
        }
    }

    fn build_metadata() {
        use std::{path::PathBuf, process::Command};
        println!("cargo:rerun-if-changed=src");
        println!("cargo:rerun-if-changed=Cargo.toml");
        println!("cargo:rerun-if-changed=LICENSE");
        println!("cargo:rerun-if-changed=THIRD_PARTY_NOTICES.txt");
        println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
        let (built, clock) = match std::env::var("SOURCE_DATE_EPOCH") {
            Ok(value) => (
                chrono::DateTime::from_timestamp(
                    value.parse().expect("SOURCE_DATE_EPOCH must be an integer"),
                    0,
                )
                .expect("SOURCE_DATE_EPOCH must be a representable UTC timestamp"),
                "SOURCE_DATE_EPOCH",
            ),
            Err(std::env::VarError::NotPresent) => (chrono::Utc::now(), "build_clock"),
            Err(error) => panic!("Cannot read SOURCE_DATE_EPOCH: {error}"),
        };
        println!(
            "cargo:rustc-env=MACCANVAS_BUILD_UTC={}",
            built.format("%Y-%m-%d %H:%M:%S UTC")
        );
        println!("cargo:rustc-env=MACCANVAS_BUILD_CLOCK={clock}");
        println!(
            "cargo:rustc-env=MACCANVAS_TARGET={}",
            std::env::var("TARGET").expect("Cargo provides TARGET")
        );
        let rustc = Command::new(std::env::var_os("RUSTC").expect("Cargo provides RUSTC"))
            .arg("--version")
            .output()
            .expect("Cannot query the Rust compiler version");
        assert!(
            rustc.status.success(),
            "Cannot query the Rust compiler version"
        );
        println!(
            "cargo:rustc-env=MACCANVAS_RUSTC={}",
            String::from_utf8(rustc.stdout)
                .expect("rustc version is UTF-8")
                .trim()
        );
        let git = |arguments: &[&str]| Command::new("git").args(arguments).output();
        let revision = match git(&["rev-parse", "HEAD"]) {
            Ok(output) if output.status.success() => {
                let revision = String::from_utf8(output.stdout)
                    .expect("Git revision is UTF-8")
                    .trim()
                    .to_owned();
                let status =
                    git(&["status", "--porcelain"]).expect("Cannot query source worktree status");
                assert!(
                    status.status.success(),
                    "Cannot query source worktree status"
                );
                format!(
                    "{revision}{}",
                    if status.stdout.is_empty() {
                        ""
                    } else {
                        "-dirty"
                    }
                )
            }
            _ => {
                println!(
                    "cargo:warning=Git source revision unavailable; this build is marked as a source archive"
                );
                "source-archive".into()
            }
        };
        println!("cargo:rustc-env=MACCANVAS_REVISION={revision}");
        if let Ok(output) = git(&["rev-parse", "--git-dir"])
            && output.status.success()
        {
            let directory = String::from_utf8(output.stdout).expect("Git directory is UTF-8");
            let directory = if cfg!(windows) {
                directory.trim().replace('/', "\\")
            } else {
                directory.trim().into()
            };
            let directory = PathBuf::from(directory);
            for path in [directory.join("HEAD"), directory.join("packed-refs")] {
                if path.exists() {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
            if let Ok(head) = std::fs::read_to_string(directory.join("HEAD"))
                && let Some(reference) = head.strip_prefix("ref: ")
            {
                let mut path = directory;
                for segment in reference.trim().split('/') {
                    path.push(segment);
                }
                if path.exists() {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
        }
    }

    fn build_brand() -> serde_json::Value {
        println!("cargo:rerun-if-changed=brand/brand.json");
        let brand: serde_json::Value =
            serde_json::from_str(include_str!("brand/brand.json")).expect("Valid brand metadata");
        assert_eq!(
            brand["confirmed"], true,
            "Unconfirmed brand cannot be packaged"
        );
        for field in [
            "app_name",
            "display_name_zh_cn",
            "publisher",
            "tagline",
            "tagline_zh_cn",
            "description",
            "executable",
            "release_name",
            "audio_asset",
            "shortcut",
            "install_receipt",
            "homepage",
            "repository",
            "repository_slug",
        ] {
            let value = brand[field]
                .as_str()
                .expect("Brand metadata must be a string");
            assert!(
                !value.is_empty() && !value.contains(['\r', '\n']),
                "Invalid brand metadata"
            );
            println!(
                "cargo:rustc-env=SOVELYX_{}={value}",
                field.to_ascii_uppercase()
            );
        }
        println!(
            "cargo:rustc-env=SOVELYX_UPDATES_READY={}",
            brand["updates_ready"]
                .as_bool()
                .expect("Explicit update policy")
        );
        brand
    }
}
