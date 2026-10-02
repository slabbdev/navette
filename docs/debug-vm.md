# Debug kit — reproduire les crashs natifs sur VM

Les deux crashs restants sont natifs plateforme (segfault GTK/WebKit sur
Linux ~10 s après le démarrage ; crash WebView2 pendant la navigation sur
Windows). Ce kit reproduit les deux sur des VM VirtualBox ARM64.

## Linux (Ubuntu 24.04 ARM64 server)

```sh
sudo apt update && sudo apt install -y \
  curl build-essential pkg-config gdb \
  libwebkit2gtk-4.1-dev libgtk-3-dev glib-networking xvfb
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source $HOME/.cargo/env
git clone https://github.com/slabbdev/navette && cd navette
cargo build --release
```

### Repro du segfault (le serve meurt ~10 s après le démarrage)

```sh
# Terminal 1 — le serve sous gdb :
gdb -ex 'handle SIGSEGV stop nopass' -ex run -ex bt --args \
  ./target/release/navette serve --port 8765
# Terminal 2 — la charge qui déclenche :
python3 -m http.server 8901 -d bench/corpus &
sleep 2
curl -sf -X POST http://127.0.0.1:8765/navigate -H 'Content-Type: application/json' \
  -d '{"url":"http://127.0.0.1:8901/bench-000.html","with_content":true}'
sleep 12   # le crash survient ~10 s après le démarrage du serve
```

Le `bt` (backtrace) au moment du segfault nomme la frame exacte
(GTK ? WebKit ? wry ?) — c'est LA donnée qui identifie le fix.

## Windows (Windows 11 ARM64 — éval Microsoft 90 jours)

```powershell
# Prérequis : git, rustup-msvc + VS Build Tools (lien C++), WebView2 runtime (préinstallé Win11)
git clone https://github.com/slabbdev/navette && cd navette
cargo build --release

# Smoke (le navigate échoue parfois silencieusement pendant la session) :
RUST_BACKTRACE=1 .\target\release\navette.exe serve --port 8765 > serve.log 2>&1
# + navigate/read via curl dans un second terminal
```

Le crash Windows survient pendant la navigation — le `serve.log`
(les breadcrumbs create_session) montre la dernière étape atteinte.

## Ce que chercher dans les backtraces

- **Linux** : la frame dans `libwebkit2gtk` / `libgtk` = le crash natif
  WebKit (le candidat : le WebProcess/DiskProc sous Xvfb).
- **Windows** : la frame dans `WebView2Loader` / `msedgewebview2` = le crash
  COM/WebView2.

Chaque backtrace = une issue GitHub avec les données exactes.
