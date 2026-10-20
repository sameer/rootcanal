//  Copyright 2022 Google, Inc.
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at:
//
//  http://www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.

use std::env;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root_dir = manifest_dir.parent().expect("manifest dir has no parent");

    generate_module(&PathBuf::from("lmp_packets.pdl").canonicalize().unwrap());
    generate_module(&PathBuf::from("llcp_packets.pdl").canonicalize().unwrap());
    generate_module(&PathBuf::from("hci_packets.pdl").canonicalize().unwrap());
    generate_module(&PathBuf::from("link_layer_packets.pdl").canonicalize().unwrap());

    #[cfg(feature = "ffi")]
    build_ffi(root_dir);

    println!("cargo:root={}", root_dir.display());
}

fn generate_module(in_file: &Path) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let mut out_file = File::create(
        out_dir
            .join(in_file.file_name().unwrap())
            .with_extension("rs"),
    )
    .unwrap();

    println!("cargo:rerun-if-changed={}", in_file.display());

    let mut sources = pdl_compiler::ast::SourceDatabase::new();
    let parsed_file = pdl_compiler::parser::parse_file(
        &mut sources,
        in_file.to_str().expect("Filename is not UTF-8"),
    )
    .expect("PDL parse failed");
    let analyzed_file = pdl_compiler::analyzer::analyze(&parsed_file).expect("PDL analysis failed");
    let rust_source = pdl_compiler::backends::rust::generate(&sources, &analyzed_file, &[]);
    out_file
        .write_all(rust_source.as_bytes())
        .expect("Could not write to output file");
}

#[cfg(feature = "ffi")]
fn generate_pdl_cxx_header(
    pdl_path: &Path,
    out_header: &Path,
    namespace: &str,
    include_header: Option<&str>,
    using_namespace: Option<&str>,
    pdl_scripts_dir: &Path,
    out_dir: &Path,
) {
    if let Some(parent) = out_header.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create parent directory for header");
    }
    let in_file_str = pdl_path.to_str().expect("Filename is not UTF-8");
    let mut sources = pdl_compiler::ast::SourceDatabase::new();
    let parsed_file =
        pdl_compiler::parser::parse_file(&mut sources, in_file_str).expect("PDL parse failed");
    let analyzed_file = pdl_compiler::analyzer::analyze(&parsed_file).expect("PDL analysis failed");
    let json_source = pdl_compiler::backends::json::generate(&analyzed_file)
        .expect("Failed to generate PDL JSON");

    let file_stem = pdl_path.file_stem().expect("pdl_path has no file stem").to_string_lossy();
    let temp_json = out_dir.join(format!("{file_stem}.json"));
    std::fs::write(&temp_json, json_source).expect("Failed to write PDL JSON");

    let mut cmd = std::process::Command::new("python3");
    cmd.arg(pdl_scripts_dir.join("generate_cxx_backend.py"))
        .arg("--input")
        .arg(&temp_json)
        .arg("--output")
        .arg(out_header)
        .arg("--namespace")
        .arg(namespace);
    if let Some(inc) = include_header {
        cmd.arg("--include-header").arg(inc);
    }
    if let Some(using) = using_namespace {
        cmd.arg("--using-namespace").arg(using);
    }
    let status = cmd.status().expect("Failed to run generate_cxx_backend.py");
    assert!(status.success(), "generate_cxx_backend.py failed for {pdl_path:?}");
}

#[cfg(feature = "ffi")]
fn build_ffi(root_dir: &Path) {
    let pdl_scripts_dir = root_dir.join("third_party/pdl/pdl-compiler/scripts");
    let fmt_dir = root_dir.join("third_party/fmtlib");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    for sub in ["include", "lib", "model", "packets", "proto"] {
        let p = root_dir.join(sub);
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }

    let proto_path = root_dir.join("proto");
    let proto_file = proto_path.join("rootcanal/configuration.proto");
    let status = std::process::Command::new("protoc")
        .arg(format!("--proto_path={}", proto_path.display()))
        .arg(format!("--cpp_out={}", out_dir.display()))
        .arg(&proto_file)
        .status()
        .expect("Failed to run protoc");
    assert!(status.success(), "protoc failed for configuration.proto");

    let hci_pdl = root_dir.join("packets/hci_packets.pdl");
    let ll_pdl = root_dir.join("packets/link_layer_packets.pdl");
    let bredr_pdl = root_dir.join("packets/bredr_bb_packets.pdl");

    generate_pdl_cxx_header(
        &hci_pdl,
        &out_dir.join("packets/hci_packets.h"),
        "bluetooth::hci",
        Some("hci/address.h"),
        None,
        &pdl_scripts_dir,
        &out_dir,
    );

    generate_pdl_cxx_header(
        &ll_pdl,
        &out_dir.join("packets/link_layer_packets.h"),
        "model::packets",
        Some("hci/address.h"),
        Some("bluetooth::hci"),
        &pdl_scripts_dir,
        &out_dir,
    );

    generate_pdl_cxx_header(
        &bredr_pdl,
        &out_dir.join("packets/bredr_bb_packets.h"),
        "bredr_bb",
        Some("hci/address.h"),
        Some("bluetooth::hci"),
        &pdl_scripts_dir,
        &out_dir,
    );

    let boringssl = boringssl_src::Build::new().build();

    let mut cxx_build = cc::Build::new();
    cxx_build
        .cpp(true)
        .std("c++20")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-deprecated-declarations")
        .flag_if_supported("-Wno-return-type")
        .include(root_dir.join("include"))
        .include(root_dir)
        .include(&out_dir)
        .include(&pdl_scripts_dir)
        .include(boringssl.include_dir())
        .include(fmt_dir.join("include"))
        .file(fmt_dir.join("src/format.cc"));

    let cpp_sources = [
        "lib/crypto/crypto.cc",
        "lib/hci/address.cc",
        "lib/log.cc",
        "model/controller/acl_connection.cc",
        "model/controller/acl_connection_handler.cc",
        "model/controller/controller_properties.cc",
        "model/controller/dual_mode_controller.cc",
        "model/controller/ffi.cc",
        "model/controller/le_acl_connection.cc",
        "model/controller/le_advertiser.cc",
        "model/controller/le_controller.cc",
        "model/controller/bredr_controller.cc",
        "model/controller/sco_connection.cc",
        "model/controller/vendor_commands/le_apcf.cc",
        "model/devices/device.cc",
    ];
    for src in &cpp_sources {
        cxx_build.file(root_dir.join(src));
    }
    cxx_build.file(out_dir.join("rootcanal/configuration.pb.cc"));
    cxx_build.compile("rootcanal_ffi");

    println!("cargo:rustc-link-search=native={}", boringssl.lib_dir().display());
    println!("cargo:rustc-link-lib=static=crypto");
    println!("cargo:rustc-link-lib=protobuf");
}
