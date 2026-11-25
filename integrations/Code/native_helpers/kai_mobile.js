"use strict";

const el = (id) => document.getElementById(id);
const tokenKey = "kai.remote.token";
const flowKey = "kai.remote.oauth";
const registrationKey = "kai.remote.client";
let token = sessionStorage.getItem(tokenKey);
let state = {};
let project = "";
let chat = null;
let task = null;
let cursor = null;
let chatOffset = null;
let listedChats = [];
let active = false;
let stream;
let pendingMessage;

function status(message, error = false) {
  el("status").textContent = message;
  el("status").classList.toggle("error", error);
}

async function request(path, { body, method = "GET", signal, headers = {} } = {}) {
  const response = await fetch(path, {
    method, body, signal, credentials: "omit", cache: "no-store",
    headers: { ...headers, ...(token ? { Authorization: "Bearer " + token } : {}) },
  });
  const data = await response.json();
  if (!response.ok || data.error || data.result?.error || data.state === "uncertain" || data.state === "pending") {
    const error = new Error(data.error || data.result?.error || (data.state ? "Kai still needs to confirm the request." : "Request failed."));
    error.uncertain = data.state === "uncertain" || data.state === "pending";
    throw error;
  }
  return data.result && data.receipt ? data.result : data;
}

function command(action, fields, requestId) {
  return request("/v1/command", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ action, ...fields, ...(requestId ? { request_id: requestId } : {}) }),
  });
}

function randomToken() {
  return base64url(crypto.getRandomValues(new Uint8Array(32)));
}

function base64url(bytes) {
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

async function pair(scope) {
  if (!isSecureContext) throw new Error("Open Kai Remote over HTTPS or a local loopback connection.");
  const redirect_uri = location.origin + "/kai/";
  let client_id = localStorage.getItem(registrationKey);
  if (!client_id) {
    const result = await request("/register", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ client_name: "Kai Remote", redirect_uris: [redirect_uri] }),
    });
    client_id = result.client_id;
    localStorage.setItem(registrationKey, client_id);
  }
  const code_verifier = randomToken();
  const challenge = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(code_verifier));
  const nonce = randomToken();
  sessionStorage.setItem(flowKey, JSON.stringify({ client_id, redirect_uri, code_verifier, nonce }));
  location.assign("/authorize?" + new URLSearchParams({
    client_id, redirect_uri, scope, state: nonce, response_type: "code",
    code_challenge_method: "S256", code_challenge: base64url(new Uint8Array(challenge)),
  }));
}

async function exchange() {
  const query = new URLSearchParams(location.search);
  if (!query.has("code")) return;
  history.replaceState(null, "", location.pathname);
  const flow = JSON.parse(sessionStorage.getItem(flowKey) || "null");
  sessionStorage.removeItem(flowKey);
  if (!flow || flow.nonce !== query.get("state")) throw new Error("Pairing session expired. Start again from Kai Remote.");
  const { client_id, redirect_uri, code_verifier } = flow;
  const grant = await request("/token", {
    method: "POST", body: new URLSearchParams({
      grant_type: "authorization_code", code: query.get("code"), client_id, redirect_uri, code_verifier,
    }),
  });
  token = grant.access_token;
  sessionStorage.setItem(tokenKey, token);
}

function button(label, onClick) {
  const control = document.createElement("button");
  control.type = "button";
  control.textContent = label;
  control.addEventListener("click", onClick);
  return control;
}

function renderChats() {
  const records = new Map();
  for (const item of [...listedChats, ...(state.chats || [])]) {
    if ((item.project || item.project_path) === project) records.set(item.id, item);
  }
  el("chats").replaceChildren(...Array.from(records.values()).map((item) => {
    const control = button(item.title || "Untitled chat", () => selectChat(item));
    if (item.id === chat?.id) control.setAttribute("aria-current", "true");
    return control;
  }));
  el("older-chats").hidden = chatOffset == null;
}

function renderSnapshot(next) {
  state = next;
  const list = state.projects || [];
  const prior = project;
  if (!list.some((item) => item.path === project)) project = list[0]?.path || "";
  const account = state.access === "control";
  el("pair").hidden = true;
  el("workspace").hidden = false;
  el("disconnect").hidden = false;
  el("new-chat").hidden = !account || !project;
  const select = el("projects");
  select.replaceChildren(...list.map((item) => new Option(item.name, item.path)));
  select.value = project;
  const previousTask = task;
  task = (state.tasks || []).find((item) => item.chat_id === chat?.id);
  el("task").textContent = task?.status || "";
  el("stop").hidden = !account || !task?.run_id || task.terminal;
  el("composer").hidden = !account || !project || !chat;
  renderChats();
  renderApprovals();
  if (project !== prior && project) loadChats(false).catch(showError);
  if (chat?.id && previousTask && !previousTask.terminal && task?.terminal) {
    loadMessages(false).catch(showError);
  }
}

function renderApprovals() {
  const cards = (state.approvals || []).filter((item) => item.owner === chat?.id);
  el("approvals").replaceChildren(...cards.map((item) => {
    const review = item.approval;
    const request = review.request;
    const box = document.createElement("section");
    box.className = "card";
    const title = document.createElement("h3");
    title.textContent = request.title || "Approval request";
    const detail = document.createElement("pre");
    detail.textContent = request.detail || "";
    box.append(title, detail);
    const choices = request.kind?.Approval?.choices || request.kind?.approval?.choices || [];
    if (state.access !== "control") return box;
    for (const choice of choices) {
      const control = button(choice.label, async () => {
        control.disabled = true;
        try {
          await command("answer", {
            chat_id: item.owner,
            answer: { approvalRequestId: review.approvalRequestId, choice: choice.id },
          }, randomToken());
          status("Decision sent.");
        } catch (error) {
          showError(error);
          control.disabled = false;
        }
      });
      box.append(control);
    }
    return box;
  }));
}

async function loadChats(more) {
  const selected = project;
  const result = await command("list_chats", { project: selected, offset: more ? chatOffset : 0 });
  if (project !== selected) return;
  listedChats = more ? listedChats.concat(result.chats || []) : result.chats || [];
  chatOffset = result.next_offset;
  renderChats();
}

function selectChat(target) {
  chat = target;
  pendingMessage = null;
  cursor = null;
  el("title").textContent = target?.title || "New chat";
  el("history").replaceChildren();
  el("refresh").hidden = !chat?.id;
  el("older").hidden = true;
  el("prompt").value = "";
  renderSnapshot(state);
  if (chat?.id) loadMessages(false).catch(showError);
  else el("prompt").focus();
}

async function loadMessages(older) {
  const id = chat?.id;
  if (!id) return;
  const result = await command("read_chat", { chat_id: id, ...(older && cursor ? { cursor } : {}) });
  if (chat?.id !== id) return;
  const messages = (result.messages || []).map((message) => {
    const row = document.createElement("article");
    const role = (message.role || "").split(":")[0];
    row.className = role === "user" ? "message user" : "message";
    const heading = document.createElement("small");
    heading.textContent = role === "user" ? "You" : "Agent";
    const body = document.createElement("pre");
    body.textContent = message.text || "";
    row.append(heading, body);
    return row;
  });
  const panel = el("history");
  if (older) {
    const before = panel.scrollHeight;
    panel.prepend(...messages);
    panel.scrollTop += panel.scrollHeight - before;
  } else {
    panel.replaceChildren(...messages);
    panel.scrollTop = panel.scrollHeight;
  }
  cursor = result.next_cursor;
  el("older").hidden = !cursor;
}

async function subscribe() {
  let retries = 0;
  while (token && stream && !stream.signal.aborted) {
    try {
      const response = await fetch("/v1/events", {
        headers: { Authorization: "Bearer " + token },
        credentials: "omit", cache: "no-store", signal: stream.signal,
      });
      if (response.status === 401) throw new Error("Pairing expired. Disconnect and pair again.");
      if (!response.ok || !response.body) throw new Error("The Kai host is unavailable.");
      const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
      let buffer = "";
      try {
        while (true) {
          const { value, done } = await reader.read();
          if (done) break;
          buffer += value;
          if (buffer.length > 3_000_000) throw new Error("The event stream returned an oversized update.");
          let end;
          while ((end = buffer.indexOf("\n\n")) >= 0) {
            const event = buffer.slice(0, end);
            buffer = buffer.slice(end + 2);
            const line = event.split("\n").find((part) => part.startsWith("data: "));
            if (!line) continue;
            const snapshot = JSON.parse(line.slice(6));
            if (!active || retries) status(snapshot.connected ? "Connected to Kai." : "Waiting for Kai on your Mac.");
            active = Boolean(snapshot.connected);
            retries = 0;
            renderSnapshot(snapshot);
          }
        }
      } finally {
        await reader.cancel().catch(() => {});
      }
    } catch (error) {
      if (!stream || stream.signal.aborted) return;
      if (error.message.startsWith("Pairing expired")) {
        showError(error);
        return;
      }
      status("Connection interrupted. Reconnecting…", true);
    }
    await new Promise((resolve) => setTimeout(resolve, Math.min(12000, 1000 * 2 ** retries++)));
  }
}

function showError(error) { status(error.message, true); }
function disconnect() {
  stream?.abort();
  stream = undefined;
  sessionStorage.removeItem(tokenKey);
  location.replace("/kai/");
}

el("connect-read").onclick = () => pair("read").catch(showError);
el("connect-control").onclick = () => pair("control").catch(showError);
el("disconnect").onclick = disconnect;
el("projects").onchange = () => {
  project = el("projects").value;
  listedChats = [];
  chatOffset = null;
  selectChat(null);
  loadChats(false).catch(showError);
};
el("older-chats").onclick = () => loadChats(true).catch(showError);
el("new-chat").onclick = () => selectChat({ title: "New chat" });
el("older").onclick = () => loadMessages(true).catch(showError);
el("refresh").onclick = () => loadMessages(false).catch(showError);
el("stop").onclick = async () => {
  if (!chat?.id || !task?.run_id) return;
  const control = el("stop");
  control.disabled = true;
  try {
    await command("stop", { chat_id: chat.id, run_id: task.run_id }, randomToken());
    status("Stop requested.");
  } catch (error) { showError(error); }
  finally { control.disabled = false; }
};

el("composer").onsubmit = async (event) => {
  event.preventDefault();
  const prompt = el("prompt").value.trim();
  if (!prompt || !chat || !project) return;
  const submit = event.submitter;
  if (submit.disabled) return;
  const owner = chat;
  const root = project;
  const payload = owner.id ? { chat_id: owner.id, prompt } : { project: root, prompt };
  const fingerprint = JSON.stringify(payload);
  if (fingerprint !== pendingMessage?.fingerprint) pendingMessage = { fingerprint, id: randomToken() };
  submit.disabled = true;
  try {
    const result = await command(owner.id ? "prompt_chat" : "create_chat", payload, pendingMessage.id);
    pendingMessage = null;
    if (owner === chat && root === project) {
      if (el("prompt").value.trim() === prompt) el("prompt").value = "";
      const id = result.chat_id ?? result.session?.id;
      if (!owner.id && id) selectChat({ id, title: prompt.slice(0, 60) });
    }
    status("Message sent.");
  } catch (error) { showError(error); }
  finally { submit.disabled = false; }
};

(async () => {
  await exchange();
  if (!token) return;
  el("disconnect").hidden = false;
  renderSnapshot(await request("/v1/snapshot"));
  status(state.connected ? "Connected to Kai." : "Waiting for Kai on your Mac.");
  stream = new AbortController();
  await subscribe();
})().catch(showError);
