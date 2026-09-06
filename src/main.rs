mod analysis;
mod baseline;
mod changelog;
mod channels;
mod cli;
mod config;
mod conventional_commits;
mod cratesio;
mod ecosystem;
mod git;
mod github;
mod manifest;
mod npm;
mod prerelease;
mod progress;
mod promotion;
mod publish;
mod pypi;
mod replacements;
mod version;
mod version_files;
mod workspace_plan;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("error: {e}");
        if std::env::var_os("RELX_VERBOSE").is_some() {
            for cause in e.chain().skip(1) {
                eprintln!("  caused by: {cause}");
            }
        }
        std::process::exit(1);
    }
}
