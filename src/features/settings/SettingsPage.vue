<script setup lang="ts">
import { onMounted, ref } from "vue";
import { open, save } from "@tauri-apps/plugin-dialog";
import { errorMessage, invoke } from "../../shared/ipc";
import SoftDialog from "../../shared/ui/SoftDialog.vue";
import type { LibraryFolder } from "../../shared/contracts/LibraryFolder";
import type { RestoreManifest } from "../../shared/contracts/RestoreManifest";
import type { OrganizedExportReport } from "../../shared/contracts/OrganizedExportReport";
import type { BackupPathMapping } from "../../shared/contracts/BackupPathMapping";
defineProps<{
  sources: LibraryFolder[];
  thumbnails: boolean;
  hideTitle: boolean;
}>();
const emit = defineEmits<{
  refresh: [];
  task: [kind: string, sourceFolderId?: number];
  "update:thumbnails": [value: boolean];
  "update:hideTitle": [value: boolean];
}>();
const section = ref("appearance"),
  busy = ref(false),
  message = ref(""),
  error = ref(""),
  dataDir = ref(""),
  confirm = ref<null | {
    title: string;
    text: string;
    run: () => Promise<void>;
  }>(null);
const pathMappings = ref<BackupPathMapping[]>([]);
const selectedMappings = () =>
  pathMappings.value
    .filter((m) => m.to.trim() && m.to.trim() !== m.from)
    .map((m) => ({ from: m.from, to: m.to.trim() }));
async function chooseMapping(mapping: BackupPathMapping) {
  const destination = await folder();
  if (destination) mapping.to = destination;
}
const settings = () => ({
  localStorage: Object.fromEntries(
    Object.keys(localStorage)
      .filter((k) => k.startsWith("illutag."))
      .map((k) => [k, localStorage.getItem(k)]),
  ),
});
async function run(work: () => Promise<void>) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  message.value = "";
  try {
    await work();
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}
async function folder() {
  const p = await open({ directory: true, multiple: false });
  return typeof p === "string" ? p : null;
}
async function file() {
  const p = await open({
    multiple: false,
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  return typeof p === "string" ? p : null;
}
function addSource() {
  void run(async () => {
    const path = await folder();
    if (!path) return;
    await invoke("add_gallery_folder_command", { folderPath: path });
    emit("refresh");
    message.value = "已登记来源。尚未扫描，请到任务中心手动处理。";
  });
}
function removeSource(source: LibraryFolder) {
  pathMappings.value = [];
  confirm.value = {
    title: "移除磁盘来源",
    text: "移除该来源的图库索引和相关归属，磁盘上的原始图片保留。完整恢复需要之前的备份。",
    run: () =>
      run(async () => {
        await invoke("remove_gallery_folder_command", {
          folderPath: source.path,
        });
        emit("refresh");
        message.value = "来源已移除";
      }),
  };
}
function backup() {
  void run(async () => {
    const destination = await folder();
    if (!destination) return;
    await invoke("create_restore_package", {
      destination,
      frontendSettings: settings(),
    });
    message.value = "完整恢复包已创建并校验。外部来源目录中的原图不包含在内。";
  });
}
function restore() {
  void run(async () => {
    const packageDir = await folder();
    if (!packageDir) return;
    const inspection = await invoke<RestoreManifest>(
      "inspect_restore_package",
      { packageDir },
    );
    pathMappings.value = (inspection.sourceDirectories ?? []).map((from) => ({
      from,
      to: from,
    }));
    confirm.value = {
      title: "恢复图库数据",
      text: `恢复包包含 ${inspection.imageCount.toLocaleString()} 张图片索引，数据版本 ${inspection.schemaVersion}。当前数据会保存为恢复前快照，恢复完成后需重启。`,
      run: () =>
        run(async () => {
          const result = await invoke<{
            frontendSettings?: { localStorage?: Record<string, string> };
          }>("restore_package", {
            packageDir,
            pathMappings: selectedMappings(),
          });
          for (const [k, v] of Object.entries(
            result.frontendSettings?.localStorage ?? {},
          ))
            localStorage.setItem(k, v);
          message.value = "已恢复，请关闭应用后重新打开。";
        }),
    };
  });
}
function jsonExport() {
  void run(async () => {
    const p = await save({
      defaultPath: "illutag-compatibility.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (p) {
      await invoke("export_data_migration_backup_command", {
        destinationPath: p,
        frontendSettings: settings(),
      });
      message.value = "兼容 JSON 已导出；它不是完整恢复包。";
    }
  });
}
function jsonImport() {
  void run(async () => {
    const p = await file();
    if (!p) return;
    const info = await invoke<{
      imageCount: number;
      pathStatuses: { path: string; exists: boolean }[];
    }>("inspect_data_migration_backup_command", { backupPath: p });
    pathMappings.value = info.pathStatuses.map(({ path }) => ({
      from: path,
      to: path,
    }));
    confirm.value = {
      title: "导入兼容 JSON",
      text: `导入 ${info.imageCount.toLocaleString()} 张图片及历史记录。请先创建完整恢复包。`,
      run: () =>
        run(async () => {
          await invoke("import_data_migration_backup_command", {
            backupPath: p,
            pathMappings: selectedMappings(),
          });
          emit("refresh");
          message.value = "兼容数据已导入";
        }),
    };
  });
}
function exportClassified() {
  void run(async () => {
    const p = await folder();
    if (p) {
      const result = await invoke<OrganizedExportReport>(
        "export_organized_folder_result_command",
        {
          destinationDir: p,
        },
      );
      message.value = `已导出 ${result.exportedCount.toLocaleString()} 项，跳过 ${result.skippedCount.toLocaleString()} 项，失败 ${result.failedCount.toLocaleString()} 项。完整清单：${result.manifestPath}`;
      if (result.failures.length) error.value = result.failures.join("\n");
    }
  });
}
function migrate() {
  pathMappings.value = [];
  void run(async () => {
    const targetDir = await folder();
    if (!targetDir) return;
    confirm.value = {
      title: "迁移图库数据目录",
      text: "数据库、偏好、配置和托管图片会复制并校验后切换。外部来源原图保留原位置。完成后需重启。",
      run: () =>
        run(async () => {
          await invoke("migrate_data_directory_command", { targetDir });
          message.value = "数据目录已迁移，请重启应用";
        }),
    };
  });
}
onMounted(() => {
  void invoke<{ dataDir: string }>("data_directory_info_command")
    .then((v) => (dataDir.value = v.dataDir))
    .catch((e) => (error.value = errorMessage(e)));
});
</script>
<template>
  <section class="page-content">
    <h1>设置</h1>
    <div class="segmented settings-tabs">
      <button
        :class="{ active: section === 'appearance' }"
        @click="section = 'appearance'"
      >
        外观与窗口</button
      ><button
        :class="{ active: section === 'storage' }"
        @click="section = 'storage'"
      >
        图库与存储</button
      ><button
        :class="{ active: section === 'backup' }"
        @click="section = 'backup'"
      >
        备份与导出
      </button>
    </div>
    <p v-if="busy" role="status">正在处理，请保持应用运行…</p>
    <p v-if="error" role="alert" class="error-text">{{ error }}</p>
    <p v-if="message" role="status" class="notice">{{ message }}</p>
    <div v-if="section === 'appearance'" class="panel settings-panel">
      <h2>浅色画廊工作台</h2>
      <p>界面跟随窗口宽度调整。图库保持图片原始比例。</p>
      <label class="setting-row"
        ><span>隐藏窗口标题栏（移动到顶端显示）</span
        ><input
          type="checkbox"
          :checked="hideTitle"
          @change="
            emit(
              'update:hideTitle',
              ($event.target as HTMLInputElement).checked,
            )
          " /></label
      ><label class="setting-row"
        ><span>优先显示已有缩略图</span
        ><input
          type="checkbox"
          :checked="thumbnails"
          @change="
            emit(
              'update:thumbnails',
              ($event.target as HTMLInputElement).checked,
            )
          "
      /></label>
      <p>切换显示方式不会自动生成缩略图。生成任务请在任务中心手动启动。</p>
    </div>
    <div v-else-if="section === 'storage'" class="panel settings-panel">
      <h2>磁盘来源目录</h2>
      <p>来源决定扫描的位置。左侧虚拟文件夹只改变分类归属。</p>
      <article v-for="source in sources" :key="source.id" class="source-row">
        <div>
          <p class="path-text">{{ source.path }}</p>
          <small>{{
            source.lastScannedAt
              ? "最近扫描：" + new Date(source.lastScannedAt).toLocaleString()
              : "尚未扫描"
          }}</small>
        </div>
        <button
          class="soft-button compact"
          @click="emit('task', 'scan', source.id)"
        >
          扫描此来源
        </button>
        <button
          class="soft-button compact"
          :disabled="busy"
          @click="removeSource(source)"
        >
          移除来源
        </button>
      </article>
      <div class="button-row">
        <button class="soft-button" :disabled="busy" @click="addSource">
          登记来源目录</button
        ><button class="soft-button" @click="emit('task', 'scan')">
          扫描来源
        </button>
      </div>
      <h2>数据目录</h2>
      <p class="path-text">{{ dataDir }}</p>
      <div class="button-row">
        <button
          class="soft-button"
          @click="invoke('open_data_directory_command')"
        >
          打开目录</button
        ><button class="soft-button" :disabled="busy" @click="migrate">
          迁移目录</button
        ><button class="soft-button" @click="emit('task', 'cleanup')">
          手动清理缺失索引
        </button>
      </div>
      <p>离线来源盘不会作为缺失文件清理。</p>
    </div>
    <div v-else class="panel settings-panel">
      <h2>完整恢复包</h2>
      <p>
        包含数据库、偏好、窗口状态、必要配置及现存托管图片。外部来源目录中的原图不包含在内。模型由便携程序包提供，缩略图可以重新生成。
      </p>
      <div class="button-row">
        <button class="soft-button accent" :disabled="busy" @click="backup">
          创建完整恢复包</button
        ><button class="soft-button" :disabled="busy" @click="restore">
          恢复完整包
        </button>
      </div>
      <h2>分类导出</h2>
      <p>按虚拟文件夹导出全部图片，不受当前界面加载数量限制。</p>
      <button class="soft-button" :disabled="busy" @click="exportClassified">
        导出分类结果
      </button>
      <h2>兼容 JSON</h2>
      <p>用于与旧版交换元数据；不是完整恢复包。</p>
      <div class="button-row">
        <button class="soft-button" :disabled="busy" @click="jsonExport">
          导出 JSON</button
        ><button class="soft-button" :disabled="busy" @click="jsonImport">
          导入 JSON
        </button>
      </div>
    </div>
    <SoftDialog v-if="confirm" :title="confirm.title" @close="confirm = null"
      ><p>{{ confirm.text }}</p>
      <div v-if="pathMappings.length" class="restore-mappings">
        <p>
          来源目录位置有变化时，可指定新的位置。保持原路径会保留已有引用；恢复包不包含这些目录的原图。
        </p>
        <label v-for="mapping in pathMappings" :key="mapping.from">
          原目录：{{ mapping.from }}
          <div class="button-row">
            <input v-model="mapping.to" aria-label="恢复后的来源目录" /><button
              class="soft-button compact"
              @click="chooseMapping(mapping)"
            >
              选择目录
            </button>
          </div>
        </label>
      </div>
      <template #actions
        ><button class="soft-button" @click="confirm = null">取消</button
        ><button
          class="soft-button accent"
          @click="
            const c = confirm;
            confirm = null;
            c.run();
          "
        >
          确认
        </button></template
      ></SoftDialog
    >
  </section>
</template>
