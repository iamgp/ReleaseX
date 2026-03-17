mod analysis;
mod changelog;
mod channels;
mod cli;
mod config;
mod conventional_commits;
mod git;
mod github;
mod progress;
mod publish;
mod pypi;
mod version;
mod version_files;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("error: {e}");
        if std::env::var_os("PYRLS_VERBOSE").is_some() {
            for cause in e.chain().skip(1) {
                eprintln!("  caused by: {cause}");
            }
        }
        std::process::exit(1);
    }
}
