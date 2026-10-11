import type { GalleryImage } from "./contracts/GalleryImage";
import type { GalleryQuery } from "./contracts/GalleryQuery";
import type { ImageMutation } from "./contracts/ImageMutation";
import type { JobRecord } from "./contracts/JobRecord";
const ratios = [
  [720, 980],
  [1200, 800],
  [800, 800],
  [900, 1500],
  [1100, 740],
  [600, 3200],
];
const colors = [
  "#c0b4c9",
  "#9ab4a9",
  "#c4aa92",
  "#95a4ba",
  "#b7bd94",
  "#bd9ca2",
];
const folders = [
  { id: 1, parentId: null, name: "风景与空间", sortOrder: 0, createdAt: 0 },
  { id: 2, parentId: null, name: "人物与故事", sortOrder: 1, createdAt: 0 },
  { id: 3, parentId: 1, name: "城市", sortOrder: 0, createdAt: 0 },
];
function svg(w: number, h: number, i: number) {
  return (
    "data:image/svg+xml;charset=utf-8," +
    encodeURIComponent(
      `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><rect width="${w}" height="${h}" fill="${colors[i % 6]}"/><circle cx="${w * 0.7}" cy="${Math.min(h * 0.22, 400)}" r="${w * 0.15}" fill="#eee3d2"/><path d="M0 ${h * 0.55} Q${w * 0.3} ${h * 0.28} ${w * 0.55} ${h * 0.56} T${w} ${h * 0.42} V${h} H0Z" fill="#5c706c"/><path d="M0 ${h * 0.75} Q${w * 0.4} ${h * 0.42} ${w} ${h * 0.82} V${h} H0Z" fill="#394b55"/><text x="28" y="${h - 32}" fill="#e8e5dc" font-size="26" font-family="sans-serif">布局预览 ${i + 1}</text></svg>`,
    )
  );
}
let images: GalleryImage[] = Array.from({ length: 48 }, (_, i) => {
  const [w, h] = ratios[i % 6];
  const path = svg(w, h, i);
  return {
    id: `preview-${i}`,
    path,
    thumbnailPath: null,
    fileName: `画面_${i + 1}.png`,
    ext: "png",
    width: w,
    height: h,
    fileSize: 1000000,
    modifiedAt: 0,
    importedAt: 0,
    folderId: 1,
    missing: false,
    trashed: false,
    isFavorite: i % 7 === 0,
    source: "gallery",
  };
});
const records: JobRecord[] = [];
let revision = 0;
export async function previewInvoke<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  let result: unknown;
  const query = (q: GalleryQuery) => {
    const rows = images
      .filter((i) => (q.scope === "trash" ? i.trashed : !i.trashed))
      .filter((i) => q.scope !== "favorites" || i.isFavorite)
      .filter((i) => !q.fileName || i.fileName.includes(q.fileName));
    return {
      images: rows.slice(q.offset, q.offset + q.limit),
      totalImageCount: rows.length,
      offset: q.offset,
      limit: q.limit,
      revision,
    };
  };
  if (command === "get_bootstrap")
    result = {
      folders: [
        {
          id: 1,
          path: "预览示例目录（无真实数据）",
          addedAt: 0,
          lastScannedAt: null,
        },
      ],
      userFolders: folders,
      page: query({ scope: "all", offset: 0, limit: 5000 } as GalleryQuery),
    };
  else if (command === "resolve_tag_labels_command")
    result = (args.tags as string[]).map((tagEn) => ({
      tagEn,
      tagZh: tagEn === "landscape" ? "风景" : null,
    }));
  else if (command === "query_gallery")
    result = query(args.query as GalleryQuery);
  else if (command === "get_image_detail")
    result = {
      image: images.find((i) => i.id === args.imageId),
      folderIds: [1],
      autoTags: {
        imageId: args.imageId,
        characterTags: [
          {
            category: "character",
            tagEn: "hatsune_miku",
            tagZh: "初音未来",
            confidence: 0.93,
          },
        ],
        generalTags: [
          ["landscape", "风景", 0.99],
          ["1girl", "1个女孩", 0.98],
          ["long_hair", "长发", 0.96],
          ["blue_eyes", "蓝色眼睛", 0.94],
          ["white_dress", "白色连衣裙", 0.93],
          ["outdoors", "户外", 0.91],
          ["sky", "天空", 0.9],
          ["cloud", "云", 0.88],
          ["backlighting", "逆光", 0.85],
          ["wind_swept_hair", "风吹起头发", 0.82],
          ["cityscape_in_background", "远处的城市背景", 0.79],
          ["a_long_untranslated_example_tag_with_several_words", null, 0.74],
          ["smile", "微笑", 0.7],
          ["from_side", "侧身姿态", 0.68],
          ["sunlight", "阳光", 0.63],
          ["looking_at_viewer", "看向观者", 0.57],
        ].map(([tagEn, tagZh, confidence]) => ({
          category: "general",
          tagEn,
          tagZh,
          confidence,
        })),
      },
      userTags: { imageId: args.imageId, customTags: [], supplementTags: [] },
    };
  else if (command === "mutate_images") {
    const r = args.request as ImageMutation;
    const successes: string[] = [];
    for (const id of r.imageIds) {
      const i = images.find((i) => i.id === id);
      if (!i) continue;
      if (r.action === "favorite") i.isFavorite = !!r.favorite;
      if (r.action === "trash") i.trashed = true;
      if (r.action === "restore") i.trashed = false;
      successes.push(id);
    }
    revision++;
    result = {
      images: images.filter((i) => successes.includes(i.id)),
      successfulIds: successes,
      failures: [],
      revision,
    };
  } else if (command === "list_jobs") result = records;
  else if (command === "start_job") {
    const r: JobRecord = {
      id: String(Date.now()),
      request: args.request as JobRecord["request"],
      state: "queued",
      stage: "等待启动",
      processed: 0,
      total: 48,
      failures: 0,
      errors: [],
      updatedAt: Date.now(),
    };
    records.unshift(r);
    result = r;
  } else if (command === "control_job") {
    const r = records.find((r) => r.id === args.id);
    if (r)
      r.state =
        args.action === "cancel"
          ? "cancelled"
          : args.action === "pause"
            ? "paused"
            : "running";
  } else if (command === "suggest_known_auto_tags_command")
    result = [{ tagEn: "landscape", tagZh: "风景", imageCount: 48 }];
  else if (command === "list_tag_management_state_command")
    result = { folders: [], unclassifiedTags: ["风景", "收藏参考"] };
  else if (command === "list_tag_dictionary_browser_command")
    result = {
      groups: [{ id: "scene", zh: "场景" }],
      tags: [
        {
          tagEn: "landscape",
          tagZh: "风景",
          group: "scene",
          imageCount: 48,
          sortIndex: 0,
        },
      ],
    };
  else if (command === "data_directory_info_command")
    result = { dataDir: "浏览器预览 · 数据隔离" };
  else throw new Error("预览模式不执行磁盘、模型或窗口操作");
  return result as T;
}
