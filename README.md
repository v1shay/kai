<div align = "center">

https://github.com/user-attachments/assets/26f9bb05-a47e-40ee-a19f-410a1ee89721

https://github.com/user-attachments/assets/ee597c1f-4ab5-4488-82f9-0f3c60db89e7

https://github.com/user-attachments/assets/2dee9871-5334-4355-ae9b-2ce6bab71b39

</div>

### setup

requires macOS 13+, Git, Swift 5.9+, Python 3.11+, and a signed-in Codex CLI

```sh
xcode-select --install
```
```sh
git --version
```
```sh
swift --version
```
```sh
python3 --version
```

### install kai

```sh
git clone https://github.com/v1shay/kai.git
cd kai

codex login
codex login status
codex app-server --help

./scripts/build-app.sh
open .build/debug/Kai.app
```

look for the 海 icon in the menu bar
- ```Command``` to dictate into your last chat
- ```Fn``` to dictate into a new chat
- ```Control``` + ```Option``` to open projects / chats 

kai uses your existing local Codex login and requires no separate account, API key, or Python packages

### permissions

for dictation, allow **Microphone** and **Speech Recognition**

if global shortcuts dont work: enable Kai under **System Settings → Privacy & Security → Accessibility** and **Input Monitoring**, then reopen

### agent CLI

`kai_cli.py` provides a scriptable non-curses interface for automated agent workflows

```sh
python3 kai_cli.py --cwd /path/to/project \
  -c '/projects /path/to/project' \
  -c '/chats' \
  --format json
```

### troubleshooting

run tests with

```sh
python3 -m unittest -v test_kai.py
```
