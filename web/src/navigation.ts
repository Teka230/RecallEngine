export type SelectionVisibility = "none" | "visible" | "outside-view";

export interface SelectionTransitionPlan {
  keepSelection: boolean;
  keepPreviousContext: boolean;
  replaceContext: boolean;
  scrollIntoView: boolean;
  outsideView: boolean;
}

export function planSelectionTransition(
  hasSelection: boolean,
  visibility: SelectionVisibility,
): SelectionTransitionPlan {
  if (!hasSelection || visibility === "none") {
    return {
      keepSelection: false,
      keepPreviousContext: false,
      replaceContext: false,
      scrollIntoView: false,
      outsideView: false,
    };
  }

  if (visibility === "visible") {
    return {
      keepSelection: true,
      keepPreviousContext: false,
      replaceContext: true,
      scrollIntoView: true,
      outsideView: false,
    };
  }

  return {
    keepSelection: true,
    keepPreviousContext: true,
    replaceContext: false,
    scrollIntoView: false,
    outsideView: true,
  };
}

export function containsMessage<T extends { message_id: string }>(
  messages: readonly T[],
  messageId: string,
): boolean {
  return messages.some((message) => message.message_id === messageId);
}

export function appendUniqueMessages<T extends { message_id: string }>(
  current: readonly T[],
  incoming: readonly T[],
): T[] {
  const seen = new Set(current.map((message) => message.message_id));
  const merged = [...current];
  for (const message of incoming) {
    if (seen.has(message.message_id)) continue;
    seen.add(message.message_id);
    merged.push(message);
  }
  return merged;
}
