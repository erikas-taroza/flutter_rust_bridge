use cargo_metadata::VersionReq;
use lazy_static::lazy_static;
use regex::Regex;
use std::fmt::Write;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use crate::command_run;
use crate::error::{Error, Result};
use crate::utils::command_runner::{call_shell, execute_command};
use crate::utils::dart_repository::dart_repo::{DartDependencyMode, DartRepository};
use log::{debug, info};

lazy_static! {
    pub(crate) static ref FFI_REQUIREMENT: VersionReq =
        VersionReq::parse(">= 2.0.1, < 3.0.0").unwrap();
    pub(crate) static ref FFIGEN_REQUIREMENT: VersionReq =
        VersionReq::parse(">= 6.0.1, < 8.0.0").unwrap();
}

pub fn ensure_tools_available(dart_root: &str, skip_deps_check: bool) -> Result {
    let repo =
        DartRepository::from_str(dart_root).map_err(|e| Error::StringError(e.to_string()))?;
    if !repo.toolchain_available() {
        return Err(Error::MissingExe(repo.toolchain.to_string()));
    }

    if !skip_deps_check {
        repo.has_specified("ffi", DartDependencyMode::Main, &FFI_REQUIREMENT)?;
        repo.has_installed("ffi", DartDependencyMode::Main, &FFI_REQUIREMENT)?;

        repo.has_specified("ffigen", DartDependencyMode::Dev, &FFIGEN_REQUIREMENT)?;
        repo.has_installed("ffigen", DartDependencyMode::Dev, &FFIGEN_REQUIREMENT)?;
    }

    Ok(())
}

pub(crate) struct BindgenRustToDartArg<'a> {
    pub rust_crate_dir: &'a str,
    pub c_output_path: &'a str,
    pub dart_output_path: &'a str,
    pub dart_class_name: &'a str,
    pub c_struct_names: Vec<String>,
    pub exclude_symbols: Vec<String>,
    pub llvm_install_path: &'a [String],
    pub llvm_compiler_opts: &'a str,
    pub prefix: &'a String,
}

pub(crate) fn bindgen_rust_to_dart(
    arg: BindgenRustToDartArg,
    dart_root: &str,
) -> anyhow::Result<()> {
    cbindgen(
        arg.rust_crate_dir,
        arg.c_output_path,
        arg.c_struct_names,
        arg.exclude_symbols,
        arg.prefix,
    )?;
    ffigen(
        arg.c_output_path,
        arg.dart_output_path,
        arg.dart_class_name,
        arg.llvm_install_path,
        arg.llvm_compiler_opts,
        dart_root,
    )
}

fn cbindgen(
    rust_crate_dir: &str,
    c_output_path: &str,
    c_struct_names: Vec<String>,
    exclude_symbols: Vec<String>,
    prefix: &String,
) -> anyhow::Result<()> {
    debug!(
        "execute cbindgen rust_crate_dir={} c_output_path={}",
        rust_crate_dir, c_output_path
    );

    let config = cbindgen::Config {
        language: cbindgen::Language::C,
        sys_includes: vec![
            "stdbool.h".to_string(),
            "stdint.h".to_string(),
            "stdlib.h".to_string(),
        ],
        no_includes: true,
        // copied from: dart-sdk/dart_api.h
        // used to convert Dart_Handle to Object.
        after_includes: Some(format!("typedef struct _Dart_Handle* {prefix}Dart_Handle;")),
        export: cbindgen::ExportConfig {
            include: c_struct_names
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect(),
            exclude: exclude_symbols,
            // This doesn't work for functions.
            prefix: Some(prefix.clone()),
            ..Default::default()
        },
        ..Default::default()
    };

    debug!("cbindgen config: {:#?}", config);

    let canonical = Path::new(rust_crate_dir)
        .canonicalize()
        .expect("Could not canonicalize rust crate dir");
    let mut path = canonical.to_str().unwrap();

    // on windows get rid of the UNC path
    if path.starts_with(r"\\?\") {
        path = &path[r"\\?\".len()..];
    }

    if cbindgen::generate_with_config(path, config)?.write_to_file(c_output_path) {
        let generated = std::fs::read_to_string(c_output_path)?;
        // This regex matches anything that needs to be prefixed.
        let regex = Regex::new(r"([\d\w]+ \*?)([\d\w]+)(\([\d\w\s*,]*\);)")?;
        let prefixed = regex.replace_all(&generated, format!("${{1}}{prefix}${{2}}${{3}}"));
        std::fs::write(c_output_path, format!("// {prefix}\n{}", prefixed.to_string()))?;

        Ok(())
    } else {
        Err(Error::string("cbindgen failed writing file").into())
    }
}

fn ffigen(
    c_path: &str,
    dart_path: &str,
    dart_class_name: &str,
    llvm_path: &[String],
    llvm_compiler_opts: &str,
    dart_root: &str,
) -> anyhow::Result<()> {
    debug!(
        "execute ffigen c_path={} dart_path={} llvm_path={:?}",
        c_path, dart_path, llvm_path
    );
    let mut config = format!(
        "
        output: '{dart_path}'
        name: '{dart_class_name}'
        description: 'generated by flutter_rust_bridge'
        headers:
          entry-points:
            - '{c_path}'
          include-directives:
            - '{c_path}'
        comments: false
        preamble: |
          // ignore_for_file: camel_case_types, non_constant_identifier_names, avoid_positional_boolean_parameters, annotate_overrides, constant_identifier_names
        "
    );
    if !llvm_path.is_empty() {
        write!(
            &mut config,
            "
        llvm-path:\n"
        )?;
        for path in llvm_path {
            writeln!(&mut config, "           - '{path}'")?;
        }
    }

    if !llvm_compiler_opts.is_empty() {
        config = format!(
            "{config}
        compiler-opts:
            - '{llvm_compiler_opts}'"
        );
    }

    debug!("ffigen config: {}", config);

    let mut config_file = tempfile::NamedTempFile::new()?;
    std::io::Write::write_all(&mut config_file, config.as_bytes())?;
    debug!("ffigen config_file: {:?}", config_file);

    let repo = DartRepository::from_str(dart_root).unwrap();
    let res = command_run!(
        call_shell[Some(dart_root)],
        *repo.toolchain.as_run_command(),
        "run",
        "ffigen",
        "--config",
        config_file.path()
    )?;
    if !res.status.success() {
        let err = String::from_utf8_lossy(&res.stderr);
        let out = String::from_utf8_lossy(&res.stdout);
        let pat = "Couldn't find dynamic library in default locations.";
        if err.contains(pat) || out.contains(pat) {
            return Err(Error::FfigenLlvm.into());
        }
        return Err(Error::string(format!("ffigen failed:\nstderr: {err}\nstdout: {out}")).into());
    }
    Ok(())
}

pub fn format_rust(path: &[PathBuf]) -> Result {
    debug!("execute format_rust path={:?}", path);
    let res = execute_command("rustfmt", path, None)?;
    if !res.status.success() {
        return Err(Error::Rustfmt(
            String::from_utf8_lossy(&res.stderr).to_string(),
        ));
    }
    Ok(())
}

pub fn format_dart(path: &[PathBuf], line_length: u32) -> Result {
    debug!(
        "execute format_dart path={:?} line_length={}",
        path, line_length
    );
    let res = command_run!(
        call_shell[None],
        "dart",
        "format",
        "--line-length",
        line_length.to_string(),
        *path
    )
    .map_err(|err| Error::StringError(format!("{err}")))?;
    if !res.status.success() {
        return Err(Error::Dartfmt(
            String::from_utf8_lossy(&res.stderr).to_string(),
        ));
    }
    Ok(())
}

pub fn build_runner(dart_root: &str) -> Result {
    info!("Running build_runner at {}", dart_root);
    let repo = DartRepository::from_str(dart_root).unwrap();
    let out = command_run!(
        call_shell[Some(dart_root)],
        *repo.toolchain.as_run_command(),
        "run",
        "build_runner",
        "build",
        "--delete-conflicting-outputs"
    )?;
    if !out.status.success() {
        return Err(Error::StringError(format!(
            "Failed to run build_runner for {}: {}",
            dart_root,
            String::from_utf8_lossy(&out.stdout)
        )));
    }
    Ok(())
}
