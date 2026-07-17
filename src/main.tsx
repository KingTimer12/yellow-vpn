import React from "react";
import ReactDOM from "react-dom/client";
import { LazyMotion, domAnimation } from "framer-motion";
import App from "./App";

// `strict` + `m.*` components (instead of `motion.*`) keep the full framer
// runtime out of the bundle; domAnimation covers all animations/gestures used.
const elem = document.getElementById("root") as HTMLElement;
const app = (
  <React.StrictMode>
    <LazyMotion features={domAnimation} strict>
      <App />
    </LazyMotion>
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
