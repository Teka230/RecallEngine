import {
  appendUniqueMessages,
  containsMessage,
  planSelectionTransition,
} from "../src/navigation.js";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function equal<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

const visible = planSelectionTransition(true, "visible");
equal(visible.keepSelection, true, "visible selection is preserved");
equal(visible.replaceContext, true, "visible selection gets target-view context");
equal(visible.scrollIntoView, true, "visible selection is scrolled into view");
equal(visible.outsideView, false, "visible selection is not marked outside view");

const outside = planSelectionTransition(true, "outside-view");
equal(outside.keepSelection, true, "outside-view selection stays inspectable");
equal(outside.keepPreviousContext, true, "outside-view selection keeps previous context");
equal(outside.scrollIntoView, false, "outside-view selection is not scrolled to a missing card");
equal(outside.outsideView, true, "outside-view state is explicit");

const none = planSelectionTransition(false, "none");
equal(none.keepSelection, false, "no selection stays empty across view changes");

const first = [{ message_id: "a" }, { message_id: "b" }];
const second = [{ message_id: "b" }, { message_id: "target" }];
const merged = appendUniqueMessages(first, second);
equal(merged.length, 3, "pagination merge removes duplicate boundary rows");
assert(containsMessage(merged, "target"), "pagination can load through the selected message");

console.log("navigation regression tests: passed");
