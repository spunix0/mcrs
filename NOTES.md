# mcrs: Minecraft Java client in Rust

Personal-use project. Goal: a faster, vanilla-compatible client. Not for redistribution (uses the user's own game files).

## Decisions
- Target version: **26.3** (unobfuscated jar, needs Java 25 to run the real game).
- Client only. Singleplayer = headless vanilla `server.jar` on localhost, client connects over the protocol.
- Renderer: custom `wgpu` + `winit` (no Bevy). Controller: `gilrs`. Menus/launcher UI: `egui`.
- Platform: Windows first, Linux later.
- Reference: Java as an oracle. Compare packets, chunk bytes and physics against the real game.

## Crates
| crate | role |
|---|---|
| mc-nbt | NBT format |
| mc-protocol | packets, framing, compression, encryption |
| mc-world | chunk storage, palettes, light |
| mc-assets | jar/asset loading, models, atlas |
| mc-render | wgpu renderer and chunk meshing |
| mc-client | game loop, input, physics |
| mc-launcher | auth, downloads, profiles, process launch |

No external dependencies yet; add them per crate as each milestone starts.

## Milestones
0. Decompile reference (done: `../reference/mc-26.3-client`, Vineflower 1.12.0 in `../tools`)
1. Launcher: login, download version, launch vanilla
2. Client joins a real server (headless done: login, config, play, keep-alive, chunks counted, chat; test server in %LOCALAPPDATA%\mcrs-server)
3. Chunks render with textures (Sodium-style mesher), benchmark vs vanilla 26.3
4. Walk with correct physics, mouse + Xbox controller
5. Entities, GUI, sound
6. One-click singleplayer via local server

## Gotchas
- Everything lives in `C:\Users\nooak\mcrs`: `build` (cargo target, see `.cargo/config.toml`), `data` (launcher installs, game dir), `server` (local test server). The project path is long and hits Windows MAX_PATH.
- Never write under AppData from the Claude desktop app: it's a packaged app, so AppData writes get redirected into its private storage (`AppData\Local\Packages\Claude_*\LocalCache`) and other programs can't see them.
- Windows PowerShell 5.1 can't list the deep `reference` folders; use Grep/Glob or `\\?\` paths.
