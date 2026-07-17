import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// Animations are driven by motion-one (see src/lib/motion.tsx) — a thin WAAPI
// wrapper, no provider component to mount here.
const elem = document.getElementById("root") as HTMLElement;
const app = (
  <React.StrictMode>
    <App />
  </React.StrictMode>
);

// Reuse the root across Bun HMR updates so hot reloads don't remount the tree
// (https://bun.com/docs/bundler/hot-reloading). In a prod bundle import.meta.hot
// is undefined, so this falls back to a plain one-time createRoot().
if (import.meta.hot) {
  import.meta.hot.data.root ??= ReactDOM.createRoot(elem);
  (import.meta.hot.data.root as ReactDOM.Root).render(app);
} else {
  ReactDOM.createRoot(elem).render(app);
}
