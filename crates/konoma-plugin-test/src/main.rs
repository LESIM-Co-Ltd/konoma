fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(konoma_plugin_test::cli::execute(
        std::env::args_os().skip(1),
        konoma_plugin_test::Limits::default(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    ))
}
