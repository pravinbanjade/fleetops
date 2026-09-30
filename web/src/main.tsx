import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

// The routed app replaces this page in Task 5.
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <p className="p-6">FleetOps</p>
  </StrictMode>,
);
