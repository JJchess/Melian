#[path = "../windows_resources.rs"]
mod windows_resources;

fn main() {
    windows_resources::embed("Melian", "ClaudeGuard.exe");
}
