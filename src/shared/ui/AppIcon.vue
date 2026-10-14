<script setup lang="ts">
import {
  Images,
  Shuffle,
  Heart,
  Inbox,
  Trash2,
  Tags,
  ListTodo,
  Settings,
  PanelLeftClose,
  Menu,
  Folder,
  FolderOpen,
  ChevronRight,
  ChevronLeft,
  ChevronDown,
  Plus,
  Ellipsis,
  X,
  Check,
  Pin,
  Minus,
  Square,
  Search,
  Play,
  Pause,
  Info,
  Copy,
  ExternalLink,
  Expand,
  CheckSquare,
  Maximize2,
  ScanSearch,
} from "@lucide/vue";
const icons = {
  images: Images,
  shuffle: Shuffle,
  heart: Heart,
  inbox: Inbox,
  trash: Trash2,
  tags: Tags,
  tasks: ListTodo,
  settings: Settings,
  collapse: PanelLeftClose,
  menu: Menu,
  folder: Folder,
  folderOpen: FolderOpen,
  right: ChevronRight,
  left: ChevronLeft,
  down: ChevronDown,
  plus: Plus,
  more: Ellipsis,
  close: X,
  check: Check,
  pin: Pin,
  minimize: Minus,
  square: Square,
  search: Search,
  play: Play,
  pause: Pause,
  info: Info,
  copy: Copy,
  external: ExternalLink,
  expand: Expand,
  select: CheckSquare,
  maximize: Maximize2,
  duplicates: ScanSearch,
};
export type IconName = keyof typeof icons;
withDefaults(
  defineProps<{ name: IconName; size?: number; filled?: boolean }>(),
  { size: 20, filled: false },
);
</script>
<template>
  <component
    :is="icons[name]"
    class="app-icon"
    :size="size"
    :stroke-width="1.8"
    :fill="filled ? 'currentColor' : 'none'"
    aria-hidden="true"
    focusable="false"
  />
</template>
