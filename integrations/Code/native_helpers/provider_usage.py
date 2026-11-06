"""Read-only provider quota collection. Only normalized windows leave this process.

Claude and OpenCode auth/endpoint handling is adapted from MonoCode rate_limits.rs
(MIT; bundled attribution). Credentials are never passed in argv, logged, or saved.
Gemini uses its CLI Code Assist quota protocol and existing signed-in OAuth token.
"""
import base64, datetime, hashlib, json, os, pathlib, re, sqlite3, struct, subprocess, sys, time, unicodedata, urllib.request

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

def proto(data):
    """Bounded protobuf fields used by Cursor's account usage response."""
    fields = {}; offset = 0
    def varint():
        nonlocal offset
        value = 0
        for shift in range(0, 70, 7):
            byte = data[offset]; offset += 1; value |= (byte & 127) << shift
            if byte < 128: return value
        raise ValueError("invalid protobuf")
    while offset < len(data):
        tag=varint(); kind=tag&7
        if kind==0: value=varint()
        elif kind==1: value=struct.unpack_from("<d",data,offset)[0]; offset+=8
        elif kind==2:
            size=varint()
            if size>len(data)-offset: raise ValueError("invalid protobuf length")
            value=data[offset:offset+size]; offset+=size
        elif kind==5: offset+=4; continue
        else: raise ValueError("invalid protobuf wire type")
        fields[tag>>3]=value
    return fields

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
    elif kind == "grok":
        auth = read(home / ".grok/auth.json")
        token = auth.get("key") or next((v.get("key") for v in auth.values() if isinstance(v, dict) and v.get("key")), None)
        if token:
            config = request("https://cli-chat-proxy.grok.com/v1/billing?format=credits", token).get("config", {})
            period = config.get("currentPeriod") or {}
            add("Weekly", config.get("creditUsagePercent", period.get("creditUsagePercent")), period.get("end"))
    elif kind in {"antigravity", "agy"}:
        executable = sys.argv[3] if len(sys.argv)>3 else ""
        if executable:
            result = subprocess.run([executable, "-p", "/usage", "--output-format", "text", "--print-timeout", "30s"], capture_output=True, timeout=34)
            if result.returncode == 0:
                for line in result.stdout.decode(errors="replace").splitlines()[:100]:
                    columns = line.split("\t")
                    if len(columns)>=4:
                        try:
                            remaining = float(columns[2].replace("%", ""))
                            if 0<=remaining<=100: add(columns[0]+" "+columns[1], 100-remaining, columns[3])
                        except ValueError: pass
    elif kind in {"cursor", "cursor-agent"}:
        folder = home / "Library/Application Support/Cursor/User/globalStorage"
        with sqlite3.connect((folder / "state.vscdb").as_uri()+"?mode=ro", uri=True, timeout=1) as db:
            row = db.execute("SELECT value FROM ItemTable WHERE key=?", ("cursorAuth/accessToken",)).fetchone()
        if not row: return []
        token = row[0]
        if token.startswith('"'): token = json.loads(token)
        storage = read(folder / "storage.json")
        machine = storage.get("telemetry.machineId")
        if not machine: return []
        stamp = int(time.time()/1000)
        previous = 165
        checksum = []
        for index, shift in enumerate([40,32,24,16,8,0]):
            previous = (((stamp >> (shift & 31)) & 255) ^ previous) + index
            previous &= 255
            checksum.append(previous)
        identity = machine + ("/"+storage["telemetry.macMachineId"] if storage.get("telemetry.macMachineId") else "")
        headers = {"Authorization":"Bearer "+token, "Content-Type":"application/proto", "Accept":"application/proto", "Connect-Protocol-Version":"1", "x-cursor-checksum":base64.urlsafe_b64encode(bytes(checksum)).decode().rstrip("=")+identity, "x-cursor-client-type":"ide", "x-cursor-client-os":"darwin", "x-cursor-client-arch":"arm64" if os.uname().machine=="arm64" else "x64"}
        req = urllib.request.Request("https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage", data=b"", headers=headers)
        with urllib.request.urlopen(req, timeout=5) as response: data=response.read(256*1024)
        fields = proto(data)
        plan = proto(fields.get(3, b""))
        used = plan.get(14)
        if plan.get(5, 0)>0 and 2 in plan: used=plan[2]/plan[5]*100
        reset = fields.get(2)
        if reset and reset>10000000000: reset/=1000
        add("Billing cycle", used, reset)
    return windows[:32]

if __name__ == "__main__":
    try:
        print(json.dumps(collect(sys.argv[1])))
    except Exception:
        # Never expose response bodies, credentials, or tracebacks to the owner UI.
        print("[]")
