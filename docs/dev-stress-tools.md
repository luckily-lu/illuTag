# 开发压力数据工具

这些 IPC 命令用于开发测试，没有用户界面入口。当前入口不会自动安装
`window.illuTagDevStress`；`src/devStressTools.ts` 只是保留的包装器。

## 先隔离运行环境

仅设置 `ILLUTAG_USE_TEST_DB=1` **不构成隔离**：它只把数据库文件名改为
`illutag.test.sqlite`，不会隔离偏好、窗口状态、WebView、日志或可写素材。
先按 [维护指南](maintenance.md#隔离开发与测试) 设置完整的隔离环境，再启动
`npm run tauri dev`。以下命令只能在这个测试窗口的 WebView 控制台执行。

首先核对实际数据目录，确认它是独立的测试目录：

```js
await window.__TAURI_INTERNALS__.invoke('data_directory_info_command')
```

这些旧开发命令并未强制要求隔离环境，也没有接入通用的只读写入保护。
不要在正式窗口或以生产库硬链接运行的性能测试窗口执行它们。

## 生成虚拟图库记录

```js
await window.__TAURI_INTERNALS__.invoke('dev_create_fake_gallery_data_command', {
  options: {
    count: 100000,
    sourceFolder: 'D:\\illuTag-dev-work\\assets\\samples',
    randomizeDimensions: true,
    randomizeFileNames: true,
    randomizeImportedAt: true,
    randomizeFolders: true,
    randomizeFavorites: true,
    randomizeTags: true,
  },
})
```

单次数量限制为 200,000。虚拟 ID 使用 `illutag-dev-fake:` 前缀，图片路径引用
少量样本并附加虚拟后缀；不会复制原图。未指定样本时，工具会读取当前测试库中
最多 100 张图片。重复生成会更新同名虚拟记录。

## 生成小文件测试集

```js
await window.__TAURI_INTERNALS__.invoke('dev_create_small_file_test_set_command', {
  options: { count: 10000, format: 'png', subfolderCount: 20 },
})
```

默认写到当前数据库旁的 `.illutag-dev-test/small-files`，生成 32×32 图片，
不自动扫描入库。`rootDir` 可指定其他绝对路径，但必须由维护者确认它属于独立
测试区；自定义位置不会被下面的默认文件清理覆盖。

## 清理工具生成的数据

```js
await window.__TAURI_INTERNALS__.invoke('dev_cleanup_stress_test_data_command')
```

这会删除当前测试库中的虚拟 ID、含 `.illutag-dev-test` 标记的图片/来源记录、
名为 `Dev Fake Folder …` 的分类，并删除数据库旁的 `.illutag-dev-test` 文件夹。
它不是通用缓存清理工具，也不会收缩数据库文件或移除 WebView 配置。
不要在生产目录使用这些保留标记；不要在生产库中运行此清理命令。
