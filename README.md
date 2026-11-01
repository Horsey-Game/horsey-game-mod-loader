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
- installs and updates the shim from the mod server on launch, checked against the signed list

the game is modded through the shim. the app downloads the latest build from the mod server when it starts. it also keeps a copy in src-tauri/resources/version.dll for when the server cant be reached and nothing is installed yet. the source for that build is on the shim branch

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

## credits

thanks to everyone who helped with this

- Kars for the icons and support
- bwhatsonn for ideas
- JumboDS64 for ideas. check out his game Cain Iddo Break Week https://store.steampowered.com/app/4068390/Cain_Iddo_Break_Week/
- cpt2angry for ideas
- everyone in the horsey game discord server