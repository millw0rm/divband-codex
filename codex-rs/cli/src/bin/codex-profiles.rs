use codex_cli::profile_manager_cmd::ProfilesCli;

fn main() -> anyhow::Result<()> {
    ProfilesCli::run_from_args()
}
