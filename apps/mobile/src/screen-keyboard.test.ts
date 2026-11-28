// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useScreenOwnsKeyboard } from "./screen-keyboard";

const drawer = vi.hoisted(() => ({ status: "closed" as "open" | "closed" }));
const keyboard = vi.hoisted(() => ({ visible: false, hidden: new Set<() => void>() }));

vi.mock("expo-router/drawer", () => ({ useDrawerStatus: () => drawer.status }));
vi.mock("react-native-keyboard-controller", () => ({
  KeyboardAvoidingView: () => null,
  KeyboardController: { isVisible: () => keyboard.visible },
  KeyboardEvents: {
    addListener: (_event: "keyboardDidHide", listener: () => void) => {
      keyboard.hidden.add(listener);
      return { remove: () => keyboard.hidden.delete(listener) };
    },
  },
}));

let root: Root;
let owns: boolean | undefined;

function Probe() {
  owns = useScreenOwnsKeyboard();
  return null;
}

function render() {
  act(() => root.render(createElement(Probe)));
}

function hideKeyboard() {
  keyboard.visible = false;
  act(() => keyboard.hidden.forEach((listener) => listener()));
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  drawer.status = "closed";
  keyboard.visible = false;
  root = createRoot(document.createElement("div"));
});

afterEach(() => act(() => root.unmount()));

describe("useScreenOwnsKeyboard", () => {
  it("lends the keyboard to the sidebar while the drawer is open", () => {
    render();
    expect(owns).toBe(true);
    drawer.status = "open";
    render();
    expect(owns).toBe(false);
  });

  it("takes the sidebar's keyboard back only once it is down, after the drawer closes", () => {
    drawer.status = "open";
    render();
    keyboard.visible = true; // The sidebar's search field raises it.
    drawer.status = "closed"; // A search result closes the drawer before the keyboard goes down.
    render();
    expect(owns).toBe(false);
    hideKeyboard();
    expect(owns).toBe(true);
  });

  it("keeps the composer's keyboard when the sidebar button opens the drawer over it", () => {
    render();
    keyboard.visible = true;
    drawer.status = "open";
    render();
    expect(owns).toBe(true);
    // Once it goes down, the next keyboard is the sidebar's.
    hideKeyboard();
    expect(owns).toBe(false);
  });
});
