import { ref, watch } from "vue";
function storedBool(key: string, fallback = false) {
  const value = localStorage.getItem(key);
  const state = ref(value === null ? fallback : value === "true");
  watch(state, (v) => localStorage.setItem(key, String(v)));
  return state;
}
export function usePreferences() {
  localStorage.setItem("illutag.themeMode", "light");
  document.documentElement.dataset.theme = "light";
  for (const key of [
    "illutag.autoScanOnStartup",
    "illutag.autoGenerateThumbnails",
    "illutag.autoNaturalLanguageScan",
  ])
    localStorage.setItem(key, "false");
  return {
    thumbnails: storedBool("illutag.thumbnailCacheEnabled"),
    collapsed: storedBool("illutag.sidebarCollapsed"),
    hideTitle: storedBool("illutag.autoHideTitlebarInWindowMode"),
    immersive: ref(false),
  };
}
