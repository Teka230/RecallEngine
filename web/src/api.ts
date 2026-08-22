import type {
  ContextResponse,
  ConversationDetail,
  ConversationSummary,
  IcResolution,
  Message,
  MetaResponse,
  Page,
  ReferenceResponse,
  SearchHit,
  ViewMode,
} from "./types";

export const API_BASE = "/v1";

export class RecallApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
  }
}

async function requestJson<T>(path: string): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    const detail = (await response.text()).trim();
    throw new RecallApiError(detail || `Request failed (${response.status})`, response.status);
  }
  return response.json() as Promise<T>;
}

function pageQuery(limit: number, cursor?: string | null): URLSearchParams {
  const params = new URLSearchParams({ limit: String(limit) });
  if (cursor) params.set("cursor", cursor);
  return params;
}

export const api = {
  meta: () => requestJson<MetaResponse>("/meta"),

  conversations: (cursor?: string | null) => {
    const params = pageQuery(60, cursor);
    return requestJson<Page<ConversationSummary>>(`/conversations?${params}`);
  },

  conversation: (conversationId: string) =>
    requestJson<ConversationDetail>(`/conversations/${encodeURIComponent(conversationId)}`),

  messages: (conversationId: string, view: ViewMode, cursor?: string | null) => {
    const params = pageQuery(100, cursor);
    params.set("view", view);
    return requestJson<Page<Message>>(
      `/conversations/${encodeURIComponent(conversationId)}/messages?${params}`,
    );
  },

  message: (messageId: string) =>
    requestJson<Message>(`/messages/${encodeURIComponent(messageId)}`),

  messageByIc: (ic: number) => requestJson<IcResolution>(`/messages/by-ic/${ic}`),

  context: (messageId: string, view: ViewMode, before = 3, after = 3) => {
    const params = new URLSearchParams({
      view,
      before: String(before),
      after: String(after),
    });
    return requestJson<ContextResponse>(
      `/messages/${encodeURIComponent(messageId)}/context?${params}`,
    );
  },

  search: (query: string, cursor?: string | null) => {
    const params = pageQuery(60, cursor);
    params.set("q", query);
    return requestJson<Page<SearchHit>>(`/search?${params}`);
  },

  reference: (reference: string) =>
    requestJson<ReferenceResponse>(`/references/${referencePath(reference)}`),
};

export type Lookup =
  | { kind: "reference"; value: string }
  | { kind: "ic"; value: number }
  | { kind: "uuid"; value: string }
  | { kind: "text"; value: string };

export function classifyLookup(raw: string): Lookup {
  const value = raw.trim();
  if (value.startsWith("ref:ic/") || (value.startsWith("[IC:") && value.endsWith("]"))) {
    return { kind: "reference", value };
  }

  const ic = value.match(/^(?:IC\s*:?\s*)?(\d+)$/i);
  if (ic) return { kind: "ic", value: Number(ic[1]) };

  if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)) {
    return { kind: "uuid", value };
  }

  return { kind: "text", value };
}

function referencePath(value: string): string {
  return value
    .trim()
    .split("/")
    .map((part) => encodeURIComponent(part))
    .join("/");
}
