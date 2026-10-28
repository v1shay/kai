"""Read-only provider quota collection. Only normalized windows leave this process.

Claude and OpenCode auth/endpoint handling is adapted from MonoCode rate_limits.rs
(MIT; bundled attribution). Credentials are never passed in argv, logged, or saved.
Gemini uses its CLI Code Assist quota protocol and existing signed-in OAuth token.
"""
import datetime, hashlib, json, os, pathlib, subprocess, sys, unicodedata, urllib.request

home = pathlib.Path(sys.argv[2]) if len(sys.argv)>2 else pathlib.Path.home()

def read(path):
    path = pathlib.Path(path)
    if path.stat().st_size > 1024 * 1024:
        raise ValueError("oversized auth file")
    return json.loads(path.read_text())

def request(url, token, body=None, headers=None):
    headers = dict(headers or {}, Authorization="Bearer " + token)
    data = None if body is None else json.dumps(body).encode()
    if data is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers)
    with urllib.request.urlopen(req, timeout=3) as response:
        return json.loads(response.read(256 * 1024))

def epoch(value):
    if isinstance(value, (int, float)):
        return int(value)
    if isinstance(value, str):
        try:
            return int(datetime.datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp())
        except ValueError:
            pass

def collect(kind):
    windows = []
    def add(label, used, reset=None, minutes=None):
        if isinstance(used, (int, float)) and 0 <= used <= 100:
            windows.append(dict(label=label, used_percent=used, resets_at=epoch(reset), duration_minutes=minutes))
    if kind == "claude":
        config = os.environ.get("CLAUDE_CONFIG_DIR")
        if sys.platform == "darwin":
            service = "Claude Code-credentials"
            if config:
                service += "-" + hashlib.sha256(unicodedata.normalize("NFC", config).encode()).hexdigest()[:8]
            raw = subprocess.run(["/usr/bin/security", "find-generic-password", "-s", service, "-w"], capture_output=True, timeout=3, check=True).stdout
            auth = json.loads(raw)
        else:
            auth = read(pathlib.Path(config) / ".credentials.json" if config else home / ".claude/.credentials.json")
        token = auth.get("claudeAiOauth", auth).get("accessToken")
        if not token:
            return []
        result = request("https://api.anthropic.com/api/oauth/usage", token, headers={"anthropic-beta": "oauth-2025-04-20", "User-Agent": "claude-code/2.1.0"})
        for key, title, minutes in [("five_hour", "5-hour limit", 300), ("seven_day", "Weekly limit", 10080), ("seven_day_sonnet", "Sonnet weekly limit", 10080), ("seven_day_opus", "Opus weekly limit", 10080)]:
            value = result.get(key) or {}
            add(title, value.get("utilization"), value.get("resets_at"), minutes)
    elif kind == "gemini":
        auth = read(home / ".gemini/oauth_creds.json")
        token = auth.get("access_token")
        if not token:
            return []
        base = "https://cloudcode-pa.googleapis.com/v1internal:"
        project = os.environ.get("GOOGLE_CLOUD_PROJECT") or os.environ.get("GOOGLE_CLOUD_PROJECT_ID")
        if not project:
            loaded = request(base + "loadCodeAssist", token, {"metadata": {"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}})
            project = loaded.get("cloudaicompanionProject")
            if isinstance(project, dict):
                project = project.get("id")
        if not project:
            return []
        result = request(base + "retrieveUserQuota", token, {"project": project})
        for value in result.get("buckets", [])[:32]:
            fraction = value.get("remainingFraction")
            if isinstance(fraction, (int, float)) and 0 <= fraction <= 1:
                add(value.get("modelId", "Gemini quota"), (1 - fraction) * 100, value.get("resetTime"))
    elif kind == "opencode":
        if os.environ.get("OPENCODE_AUTH_CONTENT"):
            auth = json.loads(os.environ["OPENCODE_AUTH_CONTENT"])
        else:
            auth = read(pathlib.Path(os.environ.get("XDG_DATA_HOME", str(home / ".local/share"))) / "opencode/auth.json")
        token = (auth.get("opencode-go") or {}).get("key")
        if not token:
            return []
        result = request("https://opencode.ai/zen/go/v1/usage", token)
        result = result.get("usage", result)
        for key, minutes in [("rolling", 300), ("weekly", 10080), ("monthly", 43200)]:
            value = result.get(key) or {}
            add(key.replace("_", " ").title(), value.get("percent", value.get("usedPercent")), value.get("resetsAt"), minutes)
    return windows[:32]

if __name__ == "__main__":
    try:
        print(json.dumps(collect(sys.argv[1])))
    except Exception:
        # Never expose response bodies, credentials, or tracebacks to the owner UI.
        print("[]")
