const API_BASE = "/v1";

const state = {
  meta: null,
  conversations: [],
  conversationCursor: null,
  activeConversation: null,
  messages: [],
  messageCursor: null,
  selectedMessage: null,
  context: null,
  view: "current-thread",
  showTechnical: false,
  leftMode: "conversations",
  searchTerm: "",
  searchResults: [],
  searchCursor: null,
};

const el = {
  searchForm: document.querySelector("#search-form"),
  searchInput: document.querySelector("#search-input"),
  status: document.querySelector("#status"),
  snapshot: document.querySelector("#snapshot-label"),
  tabConversations: document.querySelector("#tab-conversations"),
  tabSearch: document.querySelector("#tab-search"),
  leftList: document.querySelector("#left-list"),
  leftCount: document.querySelector("#left-count"),
  leftMore: document.querySelector("#left-more"),
  conversationTitle: document.querySelector("#conversation-title"),
  conversationMeta: document.querySelector("#conversation-meta"),
  viewSelect: document.querySelector("#view-select"),
  technicalToggle: document.querySelector("#technical-toggle"),
  messageList: document.querySelector("#message-list"),
  messagesMore: document.querySelector("#messages-more"),
  inspector: document.querySelector("#inspector"),
};

function setStatus(message = "", isError = false) {
  el.status.textContent = message;
  el.status.classList.toggle("error", Boolean(message) && isError);
}

async function requestJson(path) {
  const response = await fetch(`${API_BASE}${path}`, {
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    const detail = (await response.text()).trim();
    const error = new Error(detail || `Request failed (${response.status})`);
    error.status = response.status;
    throw error;
  }
  return response.json();
}

function asDate(timestamp, fallbackSeconds) {
  if (timestamp) {
    const parsed = new Date(timestamp);
    if (!Number.isNaN(parsed.valueOf())) return parsed.toLocaleString();
  }
  if (fallbackSeconds != null) {
    const parsed = new Date(fallbackSeconds * 1000);
    if (!Number.isNaN(parsed.valueOf())) return parsed.toLocaleString();
  }
  return "—";
}

function short(value, max = 96) {
  if (!value) return "—";
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function make(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function looksLikeReference(value) {
  const trimmed = value.trim();
  return trimmed.startsWith("ref:ic/") || (trimmed.startsWith("[IC:") && trimmed.endsWith("]"));
}

function referencePath(value) {
  return value
    .trim()
    .split("/")
    .map((part) => encodeURIComponent(part))
    .join("/");
}

async function loadMeta() {
  state.meta = await requestJson("/meta");
  const snapshot = state.meta.snapshot_id ? short(state.meta.snapshot_id, 44) : "No completed snapshot";
  el.snapshot.textContent = `API v${state.meta.api_schema_version} · ${snapshot}`;
  el.snapshot.title = `${state.meta.database_path}\n${state.meta.snapshot_id || "snapshot: none"}`;
}

async function loadConversations({ append = false } = {}) {
  const params = new URLSearchParams({ limit: "60" });
  if (append && state.conversationCursor) params.set("cursor", state.conversationCursor);
  const page = await requestJson(`/conversations?${params}`);
  state.conversations = append ? [...state.conversations, ...page.items] : page.items;
  state.conversationCursor = page.next_cursor;
  renderLeft();
}

async function runSearch(term, { append = false } = {}) {
  const params = new URLSearchParams({ q: term, limit: "60" });
  if (append && state.searchCursor) params.set("cursor", state.searchCursor);
  const page = await requestJson(`/search?${params}`);
  state.searchTerm = term;
  state.searchResults = append ? [...state.searchResults, ...page.items] : page.items;
  state.searchCursor = page.next_cursor;
  state.leftMode = "search";
  renderLeft();
}

async function resolveReference(reference) {
  const result = await requestJson(`/references/${referencePath(reference)}`);
  state.searchTerm = reference;
  state.searchResults = [{ ...result.message, conversation_title: "Resolved reference", excerpt: result.message.content }];
  state.searchCursor = null;
  state.leftMode = "search";
  renderLeft();
  await openMessage(result.message);
}

async function loadConversation(conversationId) {
  state.activeConversation = await requestJson(`/conversations/${encodeURIComponent(conversationId)}`);
  state.selectedMessage = null;
  state.context = null;
  await loadMessages({ append: false });
  renderReaderHeader();
  renderInspector();
}

async function loadMessages({ append = false } = {}) {
  if (!state.activeConversation) return;
  const params = new URLSearchParams({ view: state.view, limit: "100" });
  if (append && state.messageCursor) params.set("cursor", state.messageCursor);
  const page = await requestJson(
    `/conversations/${encodeURIComponent(state.activeConversation.conversation_id)}/messages?${params}`,
  );
  state.messages = append ? [...state.messages, ...page.items] : page.items;
  state.messageCursor = page.next_cursor;
  renderMessages();
}

async function loadContext(message, allowViewFallback = false) {
  const params = new URLSearchParams({ view: state.view, before: "3", after: "3" });
  try {
    state.context = await requestJson(`/messages/${encodeURIComponent(message.message_id)}/context?${params}`);
  } catch (error) {
    if (allowViewFallback && state.view === "current-thread" && error.status === 404) {
      state.view = "all-active";
      el.viewSelect.value = state.view;
      await loadMessages({ append: false });
      const fallbackParams = new URLSearchParams({ view: state.view, before: "3", after: "3" });
      state.context = await requestJson(
        `/messages/${encodeURIComponent(message.message_id)}/context?${fallbackParams}`,
      );
      setStatus("The selected message is outside the current thread; switched to All active.");
      return;
    }
    throw error;
  }
}

async function selectMessage(message, { allowViewFallback = false } = {}) {
  state.selectedMessage = message;
  state.context = null;
  renderMessages();
  renderInspector();
  await loadContext(message, allowViewFallback);
  renderInspector();
}

async function openMessage(message) {
  if (!state.activeConversation || state.activeConversation.conversation_id !== message.conversation_id) {
    await loadConversation(message.conversation_id);
  }
  const canonical = await requestJson(`/messages/${encodeURIComponent(message.message_id)}`);
  await selectMessage(canonical, { allowViewFallback: true });
  document.querySelector(`[data-message-id="${CSS.escape(canonical.message_id)}"]`)?.scrollIntoView({ block: "center" });
}

function switchLeft(mode) {
  state.leftMode = mode;
  renderLeft();
}

function renderLeft() {
  const isSearch = state.leftMode === "search";
  el.tabConversations.classList.toggle("active", !isSearch);
  el.tabSearch.classList.toggle("active", isSearch);
  const items = isSearch ? state.searchResults : state.conversations;
  el.leftCount.textContent = `${items.length}${isSearch && state.searchTerm ? ` · ${short(state.searchTerm, 28)}` : ""}`;
  el.leftList.replaceChildren();
  el.leftList.className = "list-scroll";

  if (!items.length) {
    el.leftList.classList.add("empty-state");
    el.leftList.textContent = isSearch ? "No search results." : "No conversations.";
  }

  items.forEach((item) => {
    const button = make("button", "nav-item");
    button.type = "button";
    const conversationId = item.conversation_id;
    if (state.activeConversation?.conversation_id === conversationId) button.classList.add("active");

    const title = make(
      "div",
      "nav-title",
      isSearch ? item.conversation_title || "Untitled" : item.title || "Untitled",
    );
    const meta = make("div", "nav-meta");
    if (isSearch) {
      meta.append(make("span", "", item.ic != null ? `IC ${item.ic}` : item.role || "message"));
      meta.append(make("span", "", item.model_slug || item.role || "unknown"));
    } else {
      meta.append(make("span", "", `${item.message_count} messages`));
      if (item.has_branches) meta.append(make("span", "", "branches"));
      if (item.default_model_slug) meta.append(make("span", "", item.default_model_slug));
    }
    button.append(title, meta);

    if (isSearch && item.excerpt) button.append(make("div", "nav-excerpt", item.excerpt));
    button.addEventListener("click", async () => {
      try {
        setStatus();
        if (isSearch) await openMessage(item);
        else await loadConversation(conversationId);
        renderLeft();
      } catch (error) {
        setStatus(error.message, true);
      }
    });
    el.leftList.append(button);
  });

  const cursor = isSearch ? state.searchCursor : state.conversationCursor;
  el.leftMore.classList.toggle("hidden", !cursor);
  el.leftMore.textContent = isSearch ? "Load more results" : "Load more conversations";
}

function renderReaderHeader() {
  const conversation = state.activeConversation;
  if (!conversation) {
    el.conversationTitle.textContent = "Choose a conversation";
    el.conversationMeta.textContent = "";
    return;
  }
  el.conversationTitle.textContent = conversation.title || "Untitled";
  const details = [`${conversation.message_count} messages`, conversation.default_model_slug || "model unknown"];
  if (conversation.has_branches) details.push("has branches");
  el.conversationMeta.textContent = details.join(" · ");
}

function renderMessages() {
  renderReaderHeader();
  el.messageList.replaceChildren();
  el.messageList.className = "message-list";

  if (!state.activeConversation) {
    el.messageList.classList.add("empty-state");
    el.messageList.textContent = "Select a conversation or search result to inspect its messages.";
    el.messagesMore.classList.add("hidden");
    return;
  }

  const visible = state.messages.filter((message) => state.showTechnical || !message.technical);
  if (!visible.length) {
    el.messageList.classList.add("empty-state");
    el.messageList.textContent = state.messages.length ? "Only technical messages are hidden in this page." : "No messages in this view.";
  }

  visible.forEach((message) => {
    const card = make("button", "message-card");
    card.type = "button";
    card.dataset.messageId = message.message_id;
    if (message.technical) card.classList.add("technical");
    if (state.selectedMessage?.message_id === message.message_id) card.classList.add("selected");

    const head = make("div", "message-head");
    head.append(make("span", "role", message.role));
    if (message.ic != null) head.append(make("span", "message-ref", `IC ${message.ic}`));
    if (message.model_slug) head.append(make("span", "message-ref", message.model_slug));
    if (message.technical) head.append(make("span", "badge badge-technical", "TECHNICAL"));
    if (message.branch?.is_current_thread) head.append(make("span", "badge badge-branch", "CURRENT THREAD"));
    head.append(make("span", "message-time", asDate(message.timestamp, message.created_at)));

    card.append(head, make("pre", "message-content", message.content || ""));
    card.addEventListener("click", async () => {
      try {
        setStatus();
        await selectMessage(message);
      } catch (error) {
        setStatus(error.message, true);
      }
    });
    el.messageList.append(card);
  });

  el.messagesMore.classList.toggle("hidden", !state.messageCursor);
}

function addMetaRow(grid, key, value) {
  grid.append(make("div", "meta-key", key), make("div", "meta-value", value ?? "—"));
}

function contextSection(title, items) {
  const section = make("section", "inspector-section");
  section.append(make("h3", "", title));
  if (!items?.length) {
    section.append(make("div", "reader-meta", "No messages in this side of the context window."));
    return section;
  }
  items.forEach((message) => {
    const row = make("div", "context-item");
    row.append(make("div", "context-role", `${message.role}${message.ic != null ? ` · IC ${message.ic}` : ""}`));
    row.append(make("div", "context-text", short(message.content || "", 500)));
    row.addEventListener("click", () => openMessage(message).catch((error) => setStatus(error.message, true)));
    section.append(row);
  });
  return section;
}

function renderInspector() {
  el.inspector.replaceChildren();
  el.inspector.className = "inspector-body";
  const message = state.selectedMessage;

  if (!message) {
    if (!state.activeConversation) {
      el.inspector.classList.add("empty-state");
      el.inspector.textContent = "Select a message to inspect its reference, branch and context.";
      return;
    }
    const section = make("section", "inspector-section");
    section.append(make("h3", "", "Conversation"));
    const grid = make("div", "meta-grid");
    addMetaRow(grid, "ID", state.activeConversation.conversation_id);
    addMetaRow(grid, "Current node", state.activeConversation.current_node_id);
    addMetaRow(grid, "Default model", state.activeConversation.default_model_slug);
    addMetaRow(grid, "Branches", state.activeConversation.has_branches ? "yes" : "no");
    section.append(grid);
    el.inspector.append(section);
    return;
  }

  const metaSection = make("section", "inspector-section");
  metaSection.append(make("h3", "", "Message"));
  const grid = make("div", "meta-grid");
  addMetaRow(grid, "IC", message.ic != null ? String(message.ic) : "—");
  addMetaRow(grid, "Message ID", message.message_id);
  addMetaRow(grid, "Role", message.role);
  addMetaRow(grid, "Model", message.model_slug);
  addMetaRow(grid, "Timestamp", asDate(message.timestamp, message.created_at));
  addMetaRow(grid, "Node", message.node_id);
  addMetaRow(grid, "Parent", message.parent_id);
  addMetaRow(grid, "Technical", message.technical ? "yes" : "no");
  addMetaRow(grid, "View", state.view);
  metaSection.append(grid);
  el.inspector.append(metaSection);

  const referenceSection = make("section", "inspector-section");
  referenceSection.append(make("h3", "", "Stable reference"));
  if (message.stable_reference) {
    referenceSection.append(make("div", "reference-box", message.stable_reference));
    if (message.reference) referenceSection.append(make("div", "reference-box", message.reference));
    const row = make("div", "copy-row");
    const copy = make("button", "copy-button", "Copy stable reference");
    copy.type = "button";
    copy.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(message.stable_reference);
        setStatus("Stable reference copied.");
      } catch {
        setStatus("Clipboard access is unavailable in this browser.", true);
      }
    });
    row.append(copy);
    referenceSection.append(row);
  } else {
    referenceSection.append(make("div", "reader-meta", "Technical messages do not expose a public stable reference."));
  }
  el.inspector.append(referenceSection);

  if (state.context) {
    el.inspector.append(contextSection("Before", state.context.before));
    const center = make("section", "inspector-section");
    center.append(make("h3", "", "Center"));
    center.append(make("div", "context-text", short(state.context.center.content || "", 900)));
    el.inspector.append(center);
    el.inspector.append(contextSection("After", state.context.after));
  } else {
    const loading = make("section", "inspector-section");
    loading.append(make("h3", "", "Context"), make("div", "reader-meta", "Loading context…"));
    el.inspector.append(loading);
  }
}

el.searchForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const term = el.searchInput.value.trim();
  if (!term) return;
  try {
    setStatus();
    if (looksLikeReference(term)) await resolveReference(term);
    else await runSearch(term);
  } catch (error) {
    setStatus(error.message, true);
  }
});

el.tabConversations.addEventListener("click", () => switchLeft("conversations"));
el.tabSearch.addEventListener("click", () => switchLeft("search"));

el.leftMore.addEventListener("click", async () => {
  try {
    setStatus();
    if (state.leftMode === "search") await runSearch(state.searchTerm, { append: true });
    else await loadConversations({ append: true });
  } catch (error) {
    setStatus(error.message, true);
  }
});

el.messagesMore.addEventListener("click", () => {
  loadMessages({ append: true }).catch((error) => setStatus(error.message, true));
});

el.viewSelect.addEventListener("change", async () => {
  state.view = el.viewSelect.value;
  state.selectedMessage = null;
  state.context = null;
  try {
    setStatus();
    await loadMessages({ append: false });
    renderInspector();
  } catch (error) {
    setStatus(error.message, true);
  }
});

el.technicalToggle.addEventListener("change", () => {
  state.showTechnical = el.technicalToggle.checked;
  renderMessages();
});

(async function start() {
  try {
    await Promise.all([loadMeta(), loadConversations()]);
    renderLeft();
    renderMessages();
  } catch (error) {
    setStatus(error.message, true);
    el.leftList.textContent = "RecallEngine could not load the selected database.";
  }
})();
