use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

struct Treenav;

impl zed::Extension for Treenav {
    fn new() -> Self {
        Treenav
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let env = worktree.shell_env();
        // "lsp": { "treenav": { "binary": { "path": "..." } } } wins.
        if let Some(binary) = LspSettings::for_worktree("treenav", worktree)
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
        let path = worktree.which("treenav").ok_or_else(|| {
            "treenav not found on PATH. Build it with `cargo install --path ~/projects/treenav --root ~/.local`, \
             or set lsp.treenav.binary.path in settings."
                .to_string()
        })?;
        Ok(zed::Command { command: path, args: vec![], env })
    }
}

zed::register_extension!(Treenav);
