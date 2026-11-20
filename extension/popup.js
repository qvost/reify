const byId = id => document.getElementById(id);
let snapshot;
let busy = false;

async function send(message) {
  const response = await chrome.runtime.sendMessage(message);
  if (!response?.ok) throw new Error(response?.error || "Could not reach the extension. Reopen this popup.");
  return response.data;
}

function feedback(message = "", error = false) {
  byId("feedback").textContent = message;
  byId("feedback").classList.toggle("error", error);
}

function render(state) {
  snapshot = state;
  byId("bridge-status").classList.toggle("online", state.online);
  byId("status-label").textContent = state.online
    ? "Bridge Online (127.0.0.1:9099)" : "Bridge Offline (run ‘reify bridge’)";
  byId("domain").textContent = state.tab?.domain || "Open a website";
  byId("bridge-tab").disabled = busy || !state.online || !state.paired || !state.tab;
  byId("connect").disabled = busy || !state.online;
  if (!state.paired) byId("connection").open = true;
  byId("session-count").textContent = String(state.sessions.length);
  byId("empty-state").hidden = state.sessions.length > 0;
  byId("sessions-note").hidden = !state.sessions.length || (state.online && state.paired);
  byId("session-list").replaceChildren();
  for (const session of state.sessions) {
    const row = document.createElement("li");
    row.className = "session";
    const info = document.createElement("div");
    info.className = "session-info";
    const origin = document.createElement("span");
    origin.className = "session-origin";
    origin.textContent = new URL(session.origin).host;
    origin.title = session.origin;
    const expiry = document.createElement("span");
    expiry.className = "session-expiry";
    const minutes = Math.max(1, Math.ceil((Date.parse(session.expires_at) - Date.now()) / 60000));
    expiry.textContent = `${new URL(session.origin).protocol === "https:" ? "HTTPS" : "HTTP"} · ${minutes} min left`;
    const revoke = document.createElement("button");
    revoke.className = "revoke";
    revoke.type = "button";
    revoke.textContent = "Revoke";
    revoke.setAttribute("aria-label", `Revoke session for ${new URL(session.origin).host}`);
    revoke.disabled = busy || !state.online || !state.paired;
    revoke.addEventListener("click", () => perform({ type: "REVOKE", origin: session.origin }, "Session revoked."));
    info.append(origin, expiry);
    row.append(info, revoke);
    byId("session-list").append(row);
  }
}

async function refresh() {
  if (busy) return;
  try {
    const state = await send({ type: "STATUS" });
    render(state);
    if (state.message) feedback(state.message, true);
  } catch (error) { feedback(error.message, true); }
}

async function perform(message, success) {
  if (busy) return;
  busy = true;
  if (snapshot) render(snapshot);
  feedback();
  if (message.type === "BRIDGE") byId("bridge-tab").textContent = "Bridging…";
  try {
    const state = await send(message);
    busy = false;
    render(state);
    if (message.type === "PAIR") { byId("token").value = ""; byId("connection").open = false; }
    feedback(success);
  } catch (error) {
    busy = false;
    await refresh();
    feedback(error.message, true);
  } finally {
    busy = false;
    byId("bridge-tab").textContent = "Bridge Current Tab";
  }
}

byId("bridge-tab").addEventListener("click", () => perform({ type: "BRIDGE", origin: snapshot?.tab?.origin }, "Session shared with your local gateway."));
byId("pair-form").addEventListener("submit", event => {
  event.preventDefault();
  perform({ type: "PAIR", token: byId("token").value }, "Connected to your local bridge.");
});
refresh();
setInterval(refresh, 10000);
