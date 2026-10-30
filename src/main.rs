//! The `konoma` command: the library's standard plugins, started with the process's arguments.

fn main() -> anyhow::Result<()> {
    // `run` declares this process to be the application (`konoma_base::process::mark_app_main`)
    // before anything else of konoma runs; see `Host::run`.
    konoma::Host::builder().standard_plugins().run()
}
