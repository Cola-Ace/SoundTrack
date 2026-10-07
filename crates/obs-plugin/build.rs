use std::env;
use std::path::PathBuf;

fn main() {
  println!("cargo:rerun-if-changed=wrapper.h");
  println!("cargo:rerun-if-env-changed=OBS_INCLUDE_DIRS");

  if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") {
    return;
  }
  if env::var("TARGET").ok().as_deref() != Some("x86_64-pc-windows-msvc") {
    panic!("The OBS plugin target must be x86_64-pc-windows-msvc");
  }

  let includes = env::var_os("OBS_INCLUDE_DIRS").unwrap_or_else(|| {
    panic!(
      "Set OBS_INCLUDE_DIRS to the libobs headers and generated include directories from OBS Studio 31.1.2"
    )
  });
  let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
  let mut bindings = bindgen::Builder::default()
    .header("wrapper.h")
    .clang_arg("--target=x86_64-pc-windows-msvc")
    .default_enum_style(bindgen::EnumVariation::Consts)
    .prepend_enum_name(false)
    .allowlist_type(
      "obs_source_info|obs_source_audio|obs_source_t|obs_data_t|obs_module_t|obs_properties_t|obs_property_t|obs_source_type|speaker_layout|audio_format|obs_text_type|obs_combo_type|obs_combo_format|soundtrack_obs_bindings",
    )
    .allowlist_var("SOUNDTRACK_.*")
    .allowlist_function(
      "obs_register_source_s|obs_source_output_audio|obs_data_get_string|obs_data_set_default_string|obs_properties_create|obs_properties_add_list|obs_property_list_add_string|obs_properties_add_text|obs_property_set_description|os_gettime_ns",
    )
    .layout_tests(false)
    .generate_comments(false)
    .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

  for include in env::split_paths(&includes) {
    println!("cargo:rerun-if-changed={}", include.display());
    bindings = bindings.clang_arg(format!("-I{}", include.display()));
  }

  bindings
    .generate()
    .unwrap_or_else(|error| {
      panic!("Could not generate bindings from the OBS 31.1.2 headers: {error}")
    })
    .write_to_file(output.join("obs_bindings.rs"))
    .expect("Could not write generated libobs bindings");
}
