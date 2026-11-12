import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

/** False while the window is minimized, so panes can drop their terminals. */
export function useWindowShown(): boolean {
  const [shown, setShown] = useState(true);
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const sync = () =>
      void win
        .isMinimized()
        .then((m) => alive && setShown(!m))
        .catch(() => {});
    sync();
    // Minimizing and restoring both fire a resize.
    const unlisten = win.onResized(sync);
    return () => {
      alive = false;
      void unlisten.then((f) => f());
    };
  }, []);
  return shown;
}
