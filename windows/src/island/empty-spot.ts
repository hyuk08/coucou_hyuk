// Tells an empty spot of the open island from a control, so a click there can
// fold the island like Escape. Imported before anything builds the island: it
// notes every element that listens for a press, wherever the listener comes from.

const PRESS = new Set(["click", "dblclick", "mousedown", "mouseup", "pointerdown", "pointerup"]);
const pressable = new WeakSet<Element>();

const add = EventTarget.prototype.addEventListener;
EventTarget.prototype.addEventListener = function (
  this: EventTarget,
  type: string,
  listener: EventListenerOrEventListenerObject | null,
  options?: boolean | AddEventListenerOptions,
) {
  if (PRESS.has(type) && this instanceof Element) pressable.add(this);
  add.call(this, type, listener, options);
};

// Messages and the text field's bar count as content, not as empty space.
const CONTROLS =
  "button, a, input, textarea, select, label, summary, img, canvas, [contenteditable], [role=button], [tabindex], .bubble, .reply, .chat-bar";

/** True when nothing between the clicked element and `container` (excluded) reacts to a press. */
export function isEmptySpot(e: MouseEvent, container: Element): boolean {
  // A drag that selected text is not a click on an empty spot.
  if (!(window.getSelection()?.isCollapsed ?? true)) return false;
  let el = e.target instanceof Element ? e.target : null;
  // A click on a scrollbar lands on the scrolled element, past its content box.
  if (el && (e.offsetX > el.clientWidth || e.offsetY > el.clientHeight) && el.clientWidth > 0) return false;
  for (; el && el !== container; el = el.parentElement) {
    if (pressable.has(el) || el.matches(CONTROLS)) return false;
  }
  return el === container;
}
