fn main() {
    embuild::espidf::sysenv::output();
    // The deployment's root secret is compiled in (`env!` in main.rs). Without
    // this line cargo would not notice the secret changing and could ship a
    // stale one under a fresh-looking build.
    println!("cargo:rerun-if-env-changed=OBC_SPINE_ROOT");
}
