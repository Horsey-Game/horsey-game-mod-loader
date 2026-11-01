# horsey game mod loader

this is the repo for the horsey game mod loader. the mod manager app the mod server and the shim

its split into three branches

- **manager** the windows app (tauri 2 with a win98 style ui). this is the branch you are on
- **server** the scripts that run the mod server and sign the mod list
- **shim** the version.dll proxy and lua runtime that actually runs the mods in the game

all three branches are gpl-3.0 see LICENSE

## what the mod manager does

- finds your horsey game install through steam
- installs mods from a zip or from the browse tab
- turns mods on and off without deleting them
- backs up the game folder before any change so you can always go back to vanilla
- checks the signature on the mod list before loading it so a tampered list gets refused

the game is modded through the shim. the app bundles a build of it in src-tauri/resources/version.dll so you dont need to get it separately. the source for that build is on the shim branch

## mod format

each mod is a zip with a manifest.json at the root or inside one top level folder

```json
{
  "id": "mymod",
  "name": "My Mod",
  "version": "1.0.0",
  "author": "you",
  "description": "what it does",
  "api_version": 1,
  "dependencies": [],
  "entry": "main.lua"
}
```

api_version has to be 1 right now. the id can only use letters numbers - and _

## building

you need rust and the tauri windows prerequisites. then

```
cd src-tauri
cargo build --release
```

the exe ends up in src-tauri/target/release

## the mod list signature

the app only loads the mod list if it is signed with an ed25519 key. the public key is in src-tauri/src/main.rs. the private key never goes in this repo. the server branch has the signing scripts

## status

hobby project so expect rough edges
