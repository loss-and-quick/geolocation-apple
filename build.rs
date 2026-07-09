fn main() {
    protobuf_codegen::Codegen::new()
        .pure()
        .include("protos")
        .input("protos/AppleWLoc.proto")
        .cargo_out_dir("protos")
        .run_from_script();
}
