use std::path::Path;

use regex::Regex;

use crate::config::opts::Opts;
use crate::utils::misc::{BlockIndex, PathExt};

pub fn generate_dummy(
    config: &Opts,
    all_configs: &[Opts],
    func_names: &[String],
    c_path_index: usize,
) -> String {
    let regex = Regex::new(r"wire_[\d\w]+").unwrap();
    let prefix = &config.get_unique_id();
    let func_names = &func_names
        .iter()
        .map(|e| {
            if regex.is_match(e) {
                return format!("{prefix}{e}");
            }

            e.to_string()
        })
        .collect::<Vec<String>>();

    if all_configs.len() > 1 {
        let basic_dummy_func = get_dummy_func(&config.class_name, func_names, prefix);
        if config.block_index == BlockIndex(0) {
            let func_names = all_configs
                .iter()
                .map(|e| get_dummy_signature(&e.class_name))
                .collect::<Vec<_>>();

            let other_headers = all_configs
                .iter()
                .skip(1)
                .map(|e| {
                    // get directory only from paths
                    let src_p = Path::new(&config.c_output_path[c_path_index]);
                    let dst_p = Path::new(&e.c_output_path[c_path_index]);
                    // get reletive path and header file name
                    let relative_p =
                        src_p.get_relative_path_to(dst_p.directory_name_str().unwrap(), true);
                    let header_file_name = dst_p.file_name_str().unwrap();
                    // final string for importing
                    format!(
                        r#"#include "{}""#,
                        Path::join(relative_p.as_ref(), header_file_name)
                            .into_os_string()
                            .into_string()
                            .unwrap()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "{}\n{}\n{}",
                basic_dummy_func,
                other_headers,
                get_dummy_func("", &func_names, prefix)
            )
        } else {
            basic_dummy_func
        }
    } else {
        get_dummy_func("", func_names, prefix)
    }
}

fn get_dummy_func(api_block_name: &str, func_names: &[String], prefix: &str) -> String {
    format!(
        r#"static int64_t {prefix}{signature}(void) {{
    int64_t dummy_var = 0;
{content}
    return dummy_var;
}}
"#,
        signature = get_dummy_signature(api_block_name),
        content = get_dummy_var(func_names),
    )
}

fn get_dummy_signature(api_block_name: &str) -> String {
    match api_block_name.is_empty() {
        true => "dummy_method_to_enforce_bundling".into(),
        false => format!(r#"dummy_method_to_enforce_bundling_{api_block_name}"#),
    }
}

fn get_dummy_var(func_names: &[String]) -> String {
    func_names
        .iter()
        .map(|func_name| format!("    dummy_var ^= ((int64_t) (void*) {func_name});"))
        .collect::<Vec<_>>()
        .join("\n")
}
