//! The `konoma` command: the library's standard plugins, started with the process's arguments.

fn main() -> anyhow::Result<()> {
    // The builder dispatches internal child modes first. For the normal application, `run`
    // validates the plugins and declares the process to be the application; see `Host::run`.
    konoma::Host::builder().standard_plugins().run()
}
