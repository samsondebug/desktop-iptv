import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// Desktop app: no browser context menu / text-drag behaviour on the chrome.
document.addEventListener("contextmenu", (e) => {
  const t = e.target as HTMLElement | null;
  if (!t?.closest("input, textarea")) e.preventDefault();
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
