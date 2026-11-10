use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

struct Scopehop;

impl zed::Extension for Scopehop {
    fn new() -> Self {
        Scopehop
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let env = worktree.shell_env();
        // "lsp": { "scopehop": { "binary": { "path": "..." } } } wins.
        if let Some(binary) = LspSettings::for_worktree("scopehop", worktree)
            .ok()
            .and_then(|s| s.binary)
        {
            if let Some(path) = binary.path {
                return Ok(zed::Command {
                    command: path,
                    args: binary.arguments.unwrap_or_default(),
                    env,
                });
            }
        }
        let path = worktree.which("scopehop").ok_or_else(|| {
            "scopehop not found on PATH. Build it with `cargo install --path ~/projects/scopehop --root ~/.local`, \
             or set lsp.scopehop.binary.path in settings."
                .to_string()
        })?;
        Ok(zed::Command { command: path, args: vec![], env })
    }
}

zed::register_extension!(Scopehop);
