import {
  ChangeEvent,
  FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { api, classifyLookup, RecallApiError } from "./api";
import {
  appendUniqueMessages,
  containsMessage,
  planSelectionTransition,
} from "./navigation";
import type {
  ContextResponse,
  ConversationDetail,
  ConversationSummary,
  Message,
  MetaResponse,
  SearchHit,
  SearchMode,
  ViewMode,
} from "./types";

type LeftMode = "conversations" | "search";
type Phase = "idle" | "loading" | "error" | "database-unavailable";

interface StatusState {
  phase: Phase;
  message: string;
}

interface LoadedMessages {
  items: Message[];
  nextCursor: string | null;
}

const initialStatus: StatusState = { phase: "loading", message: "Opening RecallEngine…" };

async function loadMessagesThroughTarget(
  conversationId: string,
  view: ViewMode,
  targetMessageId?: string,
): Promise<LoadedMessages> {
  let page = await api.messages(conversationId, view);
  let items = page.items;
  let cursor = page.next_cursor;
  const seenCursors = new Set<string>();

  while (
    targetMessageId &&
    !containsMessage(items, targetMessageId) &&
    cursor &&
    !seenCursors.has(cursor)
  ) {
    seenCursors.add(cursor);
    page = await api.messages(conversationId, view, cursor);
    items = appendUniqueMessages(items, page.items);
    cursor = page.next_cursor;
  }

  return { items, nextCursor: cursor };
}

export function App() {
  const [meta, setMeta] = useState<MetaResponse | null>(null);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [conversationCursor, setConversationCursor] = useState<string | null>(null);
  const [activeConversation, setActiveConversation] = useState<ConversationDetail | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [messageCursor, setMessageCursor] = useState<string | null>(null);
  const [selectedMessage, setSelectedMessage] = useState<Message | null>(null);
  const [context, setContext] = useState<ContextResponse | null>(null);
  const [selectionOutsideView, setSelectionOutsideView] = useState(false);
  const [view, setView] = useState<ViewMode>("current-thread");
  const [showTechnical, setShowTechnical] = useState(false);
  const [leftMode, setLeftMode] = useState<LeftMode>("conversations");
  const [searchInput, setSearchInput] = useState("");
  const [searchTerm, setSearchTerm] = useState("");
  const [searchResults, setSearchResults] = useState<SearchHit[]>([]);
  const [searchCursor, setSearchCursor] = useState<string | null>(null);
  const [searchMode, setSearchMode] = useState<SearchMode>("hybrid");
  const [status, setStatus] = useState<StatusState>(initialStatus);
  const messageRefs = useRef(new Map<string, HTMLButtonElement>());

  const showError = useCallback((error: unknown, fallback: string) => {
    const message = error instanceof Error ? error.message : fallback;
    const unavailable =
      error instanceof TypeError || (error instanceof RecallApiError && error.status >= 500);
    setStatus({ phase: unavailable ? "database-unavailable" : "error", message });
  }, []);

  const bootstrap = useCallback(async () => {
    setStatus({ phase: "loading", message: "Opening RecallEngine…" });
    try {
      const [nextMeta, page] = await Promise.all([api.meta(), api.conversations()]);
      setMeta(nextMeta);
      setConversations(page.items);
      setConversationCursor(page.next_cursor);
      setStatus({ phase: "idle", message: "" });
    } catch (error) {
      showError(error, "RecallEngine is unavailable.");
    }
  }, [showError]);

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  useEffect(() => {
    if (!selectedMessage || selectionOutsideView) return;
    const selectedId = selectedMessage.message_id;
    const frame = window.requestAnimationFrame(() => {
      messageRefs.current.get(selectedId)?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [messages, selectedMessage?.message_id, selectionOutsideView, view]);

  const loadConversation = useCallback(
    async (conversationId: string, target?: Message) => {
      setStatus({ phase: "loading", message: "Loading conversation…" });
      try {
        const conversation = await api.conversation(conversationId);
        let targetView = view;
        let targetContext: ContextResponse | null = null;

        if (target) {
          try {
            targetContext = await api.context(target.message_id, targetView);
          } catch (error) {
            const outsideCurrentThread =
              targetView === "current-thread" &&
              error instanceof RecallApiError &&
              error.status === 404;
            if (!outsideCurrentThread) throw error;
            targetView = "all-active";
            targetContext = await api.context(target.message_id, targetView);
          }
        }

        const loaded = await loadMessagesThroughTarget(
          conversationId,
          targetView,
          targetContext?.center.message_id ?? target?.message_id,
        );
        const nextSelection = targetContext?.center ?? target ?? null;

        setActiveConversation(conversation);
        setView(targetView);
        setMessages(loaded.items);
        setMessageCursor(loaded.nextCursor);
        setSelectedMessage(nextSelection);
        setContext(targetContext);
        setSelectionOutsideView(false);
        if (nextSelection?.technical) setShowTechnical(true);
        setStatus({ phase: "idle", message: "" });
      } catch (error) {
        showError(error, "Unable to load conversation.");
      }
    },
    [showError, view],
  );

  const openMessage = useCallback(
    async (message: Message) => {
      await loadConversation(message.conversation_id, message);
    },
    [loadConversation],
  );

  const selectMessage = useCallback(
    async (message: Message) => {
      setSelectedMessage(message);
      setSelectionOutsideView(false);
      setStatus({ phase: "loading", message: "Loading context…" });
      try {
        const nextContext = await api.context(message.message_id, view);
        setSelectedMessage(nextContext.center);
        setContext(nextContext);
        setStatus({ phase: "idle", message: "" });
      } catch (error) {
        showError(error, "Unable to load context.");
      }
    },
    [showError, view],
  );

  const runLookup = useCallback(
    async (raw: string, overrideMode?: SearchMode) => {
      const lookup = classifyLookup(raw);
      if (!lookup.value) return;
      const activeMode = overrideMode ?? searchMode;
      setStatus({ phase: "loading", message: "Searching…" });
      try {
        if (lookup.kind === "reference") {
          const resolved = await api.reference(lookup.value);
          setSearchTerm(raw.trim());
          setSearchResults([]);
          setSearchCursor(null);
          setLeftMode("search");
          await openMessage(resolved.message);
          return;
        }
        if (lookup.kind === "ic") {
          const resolved = await api.messageByIc(lookup.value);
          const message = await api.message(resolved.message_id);
          setSearchTerm(`IC ${lookup.value}`);
          setSearchResults([]);
          setSearchCursor(null);
          setLeftMode("search");
          await openMessage(message);
          return;
        }
        if (lookup.kind === "uuid") {
          const message = await api.message(lookup.value);
          setSearchTerm(lookup.value);
          setSearchResults([]);
          setSearchCursor(null);
          setLeftMode("search");
          await openMessage(message);
          return;
        }

        const page = await api.search(lookup.value, null, activeMode);
        setSearchTerm(lookup.value);
        setSearchResults(page.items);
        setSearchCursor(page.next_cursor);
        setLeftMode("search");
        setStatus({ phase: "idle", message: "" });
      } catch (error) {
        showError(error, "Nothing matched that lookup.");
      }
    },
    [openMessage, searchMode, showError],
  );

  const submitSearch = (event: FormEvent) => {
    event.preventDefault();
    void runLookup(searchInput);
  };

  const handleModeChange = (nextMode: SearchMode) => {
    if (nextMode === searchMode) return;
    setSearchMode(nextMode);
    if (searchTerm && leftMode === "search") {
      void runLookup(searchTerm, nextMode);
    }
  };

  const changeView = async (nextView: ViewMode) => {
    if (nextView === view) return;
    if (!activeConversation) {
      setView(nextView);
      return;
    }

    const previousSelection = selectedMessage;
    const previousContext = context;
    setStatus({ phase: "loading", message: "Changing branch view…" });

    try {
      if (!previousSelection) {
        const loaded = await loadMessagesThroughTarget(activeConversation.conversation_id, nextView);
        setView(nextView);
        setMessages(loaded.items);
        setMessageCursor(loaded.nextCursor);
        setSelectionOutsideView(false);
        setStatus({ phase: "idle", message: "" });
        return;
      }

      let nextContext: ContextResponse | null = null;
      let visibility: "visible" | "outside-view" = "visible";
      try {
        nextContext = await api.context(previousSelection.message_id, nextView);
      } catch (error) {
        const outsideTargetView = error instanceof RecallApiError && error.status === 404;
        if (!outsideTargetView) throw error;
        visibility = "outside-view";
      }

      const plan = planSelectionTransition(true, visibility);
      const loaded = await loadMessagesThroughTarget(
        activeConversation.conversation_id,
        nextView,
        plan.scrollIntoView ? previousSelection.message_id : undefined,
      );

      setView(nextView);
      setMessages(loaded.items);
      setMessageCursor(loaded.nextCursor);
      setSelectionOutsideView(plan.outsideView);

      if (plan.replaceContext && nextContext) {
        setSelectedMessage(nextContext.center);
        setContext(nextContext);
        if (nextContext.center.technical) setShowTechnical(true);
      } else if (!plan.keepSelection) {
        setSelectedMessage(null);
        setContext(null);
      } else if (plan.keepPreviousContext) {
        setSelectedMessage(previousSelection);
        setContext(previousContext);
      }

      setStatus({ phase: "idle", message: "" });
    } catch (error) {
      showError(error, "Unable to change branch view.");
    }
  };

  const loadMoreConversations = async () => {
    if (!conversationCursor) return;
    try {
      const page = await api.conversations(conversationCursor);
      setConversations((current) => [...current, ...page.items]);
      setConversationCursor(page.next_cursor);
    } catch (error) {
      showError(error, "Unable to load more conversations.");
    }
  };

  const loadMoreSearch = async () => {
    if (!searchCursor || !searchTerm) return;
    try {
      const page = await api.search(searchTerm, searchCursor, searchMode);
      setSearchResults((current) => [...current, ...page.items]);
      setSearchCursor(page.next_cursor);
    } catch (error) {
      showError(error, "Unable to load more search results.");
    }
  };

  const loadMoreMessages = async () => {
    if (!activeConversation || !messageCursor) return;
    try {
      const page = await api.messages(activeConversation.conversation_id, view, messageCursor);
      setMessages((current) => appendUniqueMessages(current, page.items));
      setMessageCursor(page.next_cursor);
    } catch (error) {
      showError(error, "Unable to load more messages.");
    }
  };

  const visibleMessages = useMemo(
    () => (showTechnical ? messages : messages.filter((message) => !message.technical)),
    [messages, showTechnical],
  );

  const snapshotLabel = meta?.snapshot_id ?? "No completed snapshot";

  return (
    <div className="app-shell" data-read-only="true">
      <header className="topbar">
        <div className="brand-block">
          <div className="brand-mark" aria-hidden="true">R</div>
          <div>
            <strong className="brand">RecallEngine</strong>
            <div className="subtitle">Read-only memory explorer</div>
          </div>
        </div>

        <form className="search-form" role="search" onSubmit={submitSearch}>
          <label className="sr-only" htmlFor="global-search">Search RecallEngine</label>
          <input
            id="global-search"
            value={searchInput}
            onChange={(event: ChangeEvent<HTMLInputElement>) => setSearchInput(event.target.value)}
            placeholder="Search text, IC, UUID or ref:ic/..."
            autoComplete="off"
            spellCheck={false}
          />
          <div className="search-mode-segmented" role="radiogroup" aria-label="Search mode">
            <button
              type="button"
              className={searchMode === "hybrid" ? "active" : ""}
              onClick={() => handleModeChange("hybrid")}
              title="Hybrid search (BM25 + Semantic RRF)"
            >
              Hybrid
            </button>
            <button
              type="button"
              className={searchMode === "lexical" ? "active" : ""}
              onClick={() => handleModeChange("lexical")}
              title="Lexical search (FTS5)"
            >
              Lexical
            </button>
            <button
              type="button"
              className={searchMode === "semantic" ? "active" : ""}
              onClick={() => handleModeChange("semantic")}
              title="Semantic search (Vectors)"
            >
              Semantic
            </button>
          </div>
          <button type="submit" disabled={!searchInput.trim() || status.phase === "loading"}>Search</button>
        </form>

        <div className="meta-strip" title={meta?.database_path ?? "Database unavailable"}>
          <span className="badge">READ ONLY</span>
          <span className="snapshot">API v{meta?.api_schema_version ?? "?"} · {short(snapshotLabel, 38)}</span>
        </div>
      </header>

      <StatusBanner status={status} onRetry={() => void bootstrap()} />

      <main className="workspace">
        <aside className="panel navigation-panel">
          <div className="panel-header">
            <div className="segmented">
              <button className={leftMode === "conversations" ? "active" : ""} onClick={() => setLeftMode("conversations")}>Conversations</button>
              <button className={leftMode === "search" ? "active" : ""} onClick={() => setLeftMode("search")}>Search</button>
            </div>
            <span className="count-label">{leftMode === "conversations" ? conversations.length : `${searchResults.length} (${searchMode})`}</span>
          </div>

          <div className="list-scroll">
            {leftMode === "conversations" ? (
              <ConversationList
                items={conversations}
                activeId={activeConversation?.conversation_id ?? null}
                onOpen={(id) => void loadConversation(id)}
              />
            ) : (
              <SearchList
                term={searchTerm}
                items={searchResults}
                onOpen={async (hit) => {
                  setStatus({ phase: "loading", message: "Opening result…" });
                  try {
                    await openMessage(await api.message(hit.message_id));
                  } catch (error) {
                    showError(error, "Unable to open search result.");
                  }
                }}
              />
            )}
          </div>

          {leftMode === "conversations" && conversationCursor && (
            <button className="load-more" onClick={() => void loadMoreConversations()}>Load more</button>
          )}
          {leftMode === "search" && searchCursor && (
            <button className="load-more" onClick={() => void loadMoreSearch()}>Load more</button>
          )}
        </aside>

        <section className="panel reader-panel">
          <div className="panel-header reader-header">
            <div>
              <h1>{activeConversation?.title ?? "Choose a conversation"}</h1>
              <div className="reader-meta">
                {activeConversation
                  ? `${activeConversation.message_count} messages · ${activeConversation.default_model_slug ?? "model unknown"}${activeConversation.has_branches ? " · branches" : ""}`
                  : "Search or choose a conversation to start."}
              </div>
            </div>
            <div className="reader-controls">
              <label>
                View
                <select value={view} onChange={(event: ChangeEvent<HTMLSelectElement>) => void changeView(event.target.value as ViewMode)} disabled={!activeConversation}>
                  <option value="current-thread">Current thread</option>
                  <option value="all-active">All active</option>
                </select>
              </label>
              <label className="checkbox-label">
                <input type="checkbox" checked={showTechnical} onChange={(event: ChangeEvent<HTMLInputElement>) => setShowTechnical(event.target.checked)} />
                Technical
              </label>
            </div>
          </div>

          {selectionOutsideView && selectedMessage && (
            <div className="status-banner selection-notice" role="status">
              <span>Selected message is outside this branch view. Selection and previous context are preserved.</span>
              {view === "current-thread" && (
                <button className="secondary" onClick={() => void changeView("all-active")}>Show all active</button>
              )}
            </div>
          )}

          <div className="message-list">
            {!activeConversation ? (
              <Empty>Choose a conversation or search result.</Empty>
            ) : visibleMessages.length === 0 ? (
              <Empty>No messages in this view.</Empty>
            ) : (
              visibleMessages.map((message) => (
                <button
                  key={message.message_id}
                  ref={(node: HTMLButtonElement | null) => {
                    if (node) messageRefs.current.set(message.message_id, node);
                    else messageRefs.current.delete(message.message_id);
                  }}
                  data-message-id={message.message_id}
                  className={`message-card ${selectedMessage?.message_id === message.message_id ? "selected" : ""} ${message.technical ? "technical" : ""}`}
                  onClick={() => void selectMessage(message)}
                >
                  <div className="message-head">
                    <span className={`role role-${message.role}`}>{message.role}</span>
                    {message.ic != null && <span>IC {message.ic}</span>}
                    <span>{formatDate(message.timestamp, message.created_at)}</span>
                  </div>
                  <div className="message-content">{message.content || "(empty message)"}</div>
                </button>
              ))
            )}
          </div>
          {messageCursor && <button className="load-more" onClick={() => void loadMoreMessages()}>Load more messages</button>}
        </section>

        <aside className="panel inspector-panel">
          <div className="panel-header">
            <div>
              <h2>Inspector</h2>
              <div className="reader-meta">Canonical message metadata</div>
            </div>
          </div>
          <Inspector message={selectedMessage} context={context} outsideView={selectionOutsideView} />
        </aside>
      </main>
    </div>
  );
}

function StatusBanner({ status, onRetry }: { status: StatusState; onRetry: () => void }) {
  if (status.phase === "idle") return null;
  const unavailable = status.phase === "database-unavailable";
  return (
    <div className={`status-banner ${status.phase}`} role={status.phase === "error" ? "alert" : "status"}>
      <span>{unavailable ? "Database unavailable: " : ""}{status.message}</span>
      {(status.phase === "error" || unavailable) && <button onClick={onRetry}>Retry</button>}
    </div>
  );
}

function ConversationList({
  items,
  activeId,
  onOpen,
}: {
  items: ConversationSummary[];
  activeId: string | null;
  onOpen: (id: string) => void;
}) {
  if (items.length === 0) return <Empty>No conversations.</Empty>;
  return <>{items.map((item) => (
    <button key={item.conversation_id} className={`list-item ${activeId === item.conversation_id ? "active" : ""}`} onClick={() => onOpen(item.conversation_id)}>
      <strong>{item.title || "Untitled"}</strong>
      <span>{item.message_count} messages · {formatDate(null, item.updated_at)}</span>
      <span>{item.default_model_slug ?? "model unknown"}{item.has_branches ? " · branches" : ""}</span>
    </button>
  ))}</>;
}

function SearchList({
  term,
  items,
  onOpen,
}: {
  term: string;
  items: SearchHit[];
  onOpen: (hit: SearchHit) => Promise<void>;
}) {
  if (!term) return <Empty>Search text, an IC, UUID or stable reference.</Empty>;
  if (items.length === 0) return <Empty>No list results. Direct IC/UUID/reference lookups open the message immediately.</Empty>;
  return <>{items.map((hit) => (
    <button key={hit.message_id} className="list-item" onClick={() => void onOpen(hit)}>
      <strong>{hit.conversation_title || "Untitled"}</strong>
      <span>{hit.role}{hit.ic != null ? ` · IC ${hit.ic}` : ""} · {hit.model_slug ?? "model unknown"}</span>
      <span>{short(hit.excerpt, 150)}</span>
    </button>
  ))}</>;
}

function Inspector({
  message,
  context,
  outsideView,
}: {
  message: Message | null;
  context: ContextResponse | null;
  outsideView: boolean;
}) {
  if (!message) return <div className="inspector-body"><Empty>Select a message to inspect it.</Empty></div>;
  const copyReference = async () => {
    if (!message.stable_reference) return;
    await navigator.clipboard.writeText(message.stable_reference);
  };
  const branchLabel = outsideView
    ? "outside current view"
    : message.branch.is_current_thread === true
      ? "current thread"
      : "active branch";

  return (
    <div className="inspector-body">
      <dl className="metadata-grid">
        <Meta label="IC" value={message.ic?.toString() ?? "—"} />
        <Meta label="UUID" value={message.message_id} mono />
        <Meta label="Model" value={message.model_slug ?? "—"} />
        <Meta label="Role" value={message.role} />
        <Meta label="Node" value={message.node_id} mono />
        <Meta label="Parent" value={message.parent_id ?? "—"} mono />
        <Meta label="Branch" value={branchLabel} />
        <Meta label="Timestamp" value={formatDate(message.timestamp, message.created_at)} />
      </dl>

      <section className="inspector-section">
        <div className="section-title">Stable reference</div>
        <code className="reference-block">{message.stable_reference ?? "No public reference for technical messages"}</code>
        <button className="secondary" disabled={!message.stable_reference} onClick={() => void copyReference()}>Copy reference</button>
      </section>

      <section className="inspector-section">
        <div className="section-title">Assets</div>
        {message.assets.length === 0 ? <span className="muted">No linked assets.</span> : (
          <ul className="asset-list">{message.assets.map((asset) => (
            <li key={asset.id}><strong>{asset.name}</strong><span>{asset.mime_type ?? "unknown"} · {asset.exists_locally ? "local" : "missing"}</span></li>
          ))}</ul>
        )}
      </section>

      <section className="inspector-section">
        <div className="section-title">Context</div>
        {outsideView && <span className="muted">Context shown is preserved from the previous branch view.</span>}
        {!context ? <span className="muted">Loading or unavailable.</span> : (
          <div className="context-stack">
            {context.before.map((item) => <ContextLine key={item.message_id} message={item} />)}
            <ContextLine message={context.center} center />
            {context.after.map((item) => <ContextLine key={item.message_id} message={item} />)}
          </div>
        )}
      </section>
    </div>
  );
}

function ContextLine({ message, center = false }: { message: Message; center?: boolean }) {
  return <div className={`context-line ${center ? "center" : ""}`}>
    <span>{message.role}{message.ic != null ? ` · IC ${message.ic}` : ""}</span>
    <p>{short(message.content || "(empty)", 180)}</p>
  </div>;
}

function Meta({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return <div><dt>{label}</dt><dd className={mono ? "mono" : ""} title={value}>{value}</dd></div>;
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="empty-state">{children}</div>;
}

function short(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function formatDate(timestamp: string | null, seconds: number | null): string {
  if (timestamp) {
    const date = new Date(timestamp);
    if (!Number.isNaN(date.valueOf())) return date.toLocaleString();
  }
  if (seconds != null) {
    const date = new Date(seconds * 1000);
    if (!Number.isNaN(date.valueOf())) return date.toLocaleString();
  }
  return "—";
}
