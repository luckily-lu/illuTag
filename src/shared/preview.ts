import type { GalleryImage } from "./contracts/GalleryImage";
import type { GalleryQuery } from "./contracts/GalleryQuery";
import type { ImageMutation } from "./contracts/ImageMutation";
import type { JobRecord } from "./contracts/JobRecord";
import type { DuplicateStatus } from "./contracts/DuplicateStatus";
import type { DuplicateGroup } from "./contracts/DuplicateGroup";
import type { DuplicateOptions } from "./contracts/DuplicateOptions";
import type { DuplicateQuery } from "./contracts/DuplicateQuery";
import type { DuplicateMemberQuery } from "./contracts/DuplicateMemberQuery";
import type { DuplicateWorkflowStatus } from "./contracts/DuplicateWorkflowStatus";
import type { DuplicateWorkflowGroup } from "./contracts/DuplicateWorkflowGroup";
import type { DuplicateWorkflowQuery } from "./contracts/DuplicateWorkflowQuery";
import type { DuplicateReviewEdit } from "./contracts/DuplicateReviewEdit";
import type { DuplicateWorkflowRequest } from "./contracts/DuplicateWorkflowRequest";
import type { DuplicateActionPreview } from "./contracts/DuplicateActionPreview";
import type { DuplicateOperationStatus } from "./contracts/DuplicateOperationStatus";
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
const favoriteTags = new Set<string>();
const previewParams = new URLSearchParams(location.search);
const layoutTags = previewParams.get("tags") === "layout";
const snapshotFixture = previewParams.get("snapshot");
let snapshotReads = 0,
  snapshotRetried = false;
const taskActive = ["queued", "running", "pausing", "paused", "cancelling"];
function taskMeta(job: JobRecord) {
  const old = job.meta;
  job.meta = {
    revision: (old?.revision ?? 0) + 1,
    createdAt: old?.createdAt ?? job.updatedAt,
    startedAt: old?.startedAt ?? job.updatedAt,
    ...(!taskActive.includes(job.state) ? { finishedAt: Date.now() } : {}),
    stageCode: job.request.kind,
    unit: "images",
    canPause: job.state === "running",
    canResume: job.state === "paused",
    canCancel: ["queued", "running", "pausing", "paused"].includes(job.state),
    canRetry: ["failed", "partial", "interrupted", "cancelled"].includes(
      job.state,
    ),
  };
}
const taskFixture = new URLSearchParams(location.search).get("tasks");
if (["ready", "save"].includes(taskFixture ?? "")) {
  for (let n = 0; n < 47; n++) {
    const state =
      n === 0
        ? "running"
        : n < 3
          ? "queued"
          : n % 6 === 0
            ? "failed"
            : n % 7 === 0
              ? "interrupted"
              : "completed";
    const job: JobRecord = {
      id: `task-${String(n).padStart(3, "0")}`,
      request: {
        kind:
          n === 0
            ? "tagging"
            : n === 1
              ? "scan"
              : n === 2
                ? "semantic"
                : n % 2
                  ? "thumbnails"
                  : "scan",
        thumbnails: true,
        sourceFolderId: n % 3 === 0 ? 1 : null,
        duplicateOptions: null,
        duplicateOperationId: null,
      },
      state,
      stage: n === 0 ? "自动打标" : "scan",
      processed: state === "queued" ? 0 : n === 0 ? 1280 : 720,
      total: state === "queued" ? 0 : n === 0 ? 10000 : 720,
      failures: state === "failed" ? 3 : 0,
      errors:
        state === "failed"
          ? ["合成示例：素材盘暂时不可用，已保留完成结果。"]
          : [],
      updatedAt: Date.now() - n * 60000,
    };
    taskMeta(job);
    if (n === 0 && taskFixture === "save") {
      job.meta!.persistenceError = "合成示例：database is locked";
      job.meta!.canRetry = true;
      job.meta!.canPause = false;
      job.meta!.canCancel = false;
    }
    records.push(job);
  }
}
let revision = 0;
let duplicateFixtureState =
  new URLSearchParams(location.search).get("duplicates") || "empty";
let duplicateGeneration =
  duplicateFixtureState === "empty" ? null : "preview-1";
let duplicateSensitivity = "standard";
let duplicateVerificationMode = "candidate";
let duplicateSequence = 1;
let workflowStage = duplicateGeneration
  ? duplicateFixtureState === "refined"
    ? "refined"
    : "scanned"
  : "empty";
let workflowRevision = 1,
  lastWorkflowOperation: string | null = null;
const reviewChoices = new Map<string, string>(),
  reviewKeepers = new Map<string, string>();
const confirmedGroups = new Set<string>(),
  duplicateTrashed = new Set<string>();
const workflowOperations = new Map<
  string,
  {
    preview: DuplicateActionPreview;
    status: DuplicateOperationStatus;
    targets: string[];
  }
>();
const duplicateFixtures: DuplicateGroup[] = Array.from(
  { length: 28 },
  (_, index) => {
    const kind = index < 14 ? "exact" : "near";
    const sample = images[index % images.length];
    const members = Array.from(
      { length: index === 0 ? 80 : 8 },
      (_, member) => ({
        image: {
          ...sample,
          id: `preview-duplicate-${index}-${member}`,
          fileName: `${member === 0 ? "参考图" : "副本"}_${index + 1}_${member + 1}.png`,
        },
        metric: {
          phashDistance: kind === "near" ? member % 5 : null,
          dhashDistance: kind === "near" ? member % 8 : null,
          colorDistance: kind === "near" ? 0.035 : null,
        },
        stale: false,
      }),
    );
    return {
      id: `fixture-${index}`,
      kind,
      reference: members[0],
      totalMembers: members.length,
      members,
      nextCursor: null,
      needsReview: false,
    };
  },
);
function duplicateStatusFixture(): DuplicateStatus {
  const coverage = {
    total: 296,
    processed: 296,
    failed: 0,
    skippedNear: 3,
    lowInformation: 2,
    tooDense: 0,
    verifiedPairs: 0,
    contentVariants: 0,
    uncertainPairs: 0,
  };
  return {
    generation: duplicateGeneration,
    sensitivity: duplicateSensitivity,
    verificationMode: duplicateVerificationMode,
    checkedAt: duplicateGeneration ? Date.now() : null,
    durationMs: duplicateGeneration ? 12300 : null,
    state:
      duplicateFixtureState === "refined"
        ? "ready"
        : duplicateFixtureState === "empty"
          ? "empty"
          : duplicateFixtureState,
    needsUpdate: ["stale", "invalid"].includes(duplicateFixtureState),
    coverage: duplicateGeneration
      ? coverage
      : {
          total: 296,
          processed: 0,
          failed: 0,
          skippedNear: 0,
          lowInformation: 0,
          tooDense: 0,
          verifiedPairs: 0,
          contentVariants: 0,
          uncertainPairs: 0,
        },
    cacheBytes: duplicateGeneration ? 8 * 1024 * 1024 : 0,
    exactGroups: duplicateGeneration ? 14 : 0,
    nearGroups: duplicateGeneration ? 14 : 0,
    uniqueImages: duplicateGeneration ? 296 : 0,
    errors:
      duplicateFixtureState === "invalid"
        ? ["预览示例：缓存校验失败，请重新建立索引。"]
        : [],
    lastAttempt:
      duplicateFixtureState === "partial"
        ? {
            state: "partial",
            coverage: { ...coverage, processed: 180, failed: 2 },
            updatedAt: Date.now(),
            errors: ["预览示例：来源目录离线，保留上一次完整结果。"],
          }
        : null,
  };
}
function workflowKind(group: DuplicateGroup) {
  if (group.kind === "exact") return "exact";
  const number = Number(group.id.split("-").at(-1));
  return workflowStage !== "refined"
    ? "candidate"
    : number % 3 === 0
      ? "near"
      : number % 3 === 1
        ? "similar"
        : "uncertain";
}
function workflowGroup(
  group: DuplicateGroup,
  offset = 0,
  limit = 50,
): DuplicateWorkflowGroup {
  const kind = workflowKind(group),
    keeper = reviewKeepers.get(group.id) ?? group.reference.image.id;
  const members = group.members.map((member) => {
    const choice = reviewChoices.get(member.image.id);
    const isKeeper = member.image.id === keeper;
    const disposition = duplicateTrashed.has(member.image.id)
      ? "keep"
      : (choice ??
        (isKeeper
          ? "keep"
          : workflowStage !== "refined" && kind !== "exact"
            ? "pending"
            : ["exact", "near"].includes(kind)
              ? "clean"
              : "keep"));
    return {
      member: {
        ...member,
        image: {
          ...member.image,
          trashed: duplicateTrashed.has(member.image.id),
        },
      },
      disposition,
      isKeeper,
      reason: duplicateTrashed.has(member.image.id)
        ? "已移入应用回收站"
        : choice === "protected"
          ? "本次人工保留"
          : choice === "manual"
            ? "人工标记，非算法确认"
            : kind === "uncertain"
              ? "局部差异无法确认，默认保留"
              : "",
    };
  });
  return {
    id: group.id,
    kind,
    referenceId: keeper,
    confirmed: confirmedGroups.has(group.id),
    totalMembers: members.length,
    members: members.slice(offset, offset + limit),
    nextCursor: offset + limit < members.length ? String(offset + limit) : null,
  };
}
function workflowTargets() {
  if (workflowStage !== "refined") return [];
  const targets = new Set<string>();
  for (const group of duplicateFixtures)
    for (const row of workflowGroup(group, 0, 1000).members)
      if (
        !row.isKeeper &&
        ["clean", "manual"].includes(row.disposition) &&
        !duplicateTrashed.has(row.member.image.id)
      )
        targets.add(row.member.image.id);
  for (const group of duplicateFixtures)
    targets.delete(reviewKeepers.get(group.id) ?? group.reference.image.id);
  for (const [id, choice] of reviewChoices)
    if (choice === "protected") targets.delete(id);
  return [...targets];
}
function workflowStatusFixture(): DuplicateWorkflowStatus {
  const targets = new Set(workflowTargets());
  const kinds = [
    "exact",
    "candidate",
    "near",
    "similar",
    "uncertain",
    "suggested",
    "recycle",
    "retained",
  ];
  return {
    generation: duplicateGeneration,
    policy: workflowStage === "refined" ? "illuTag-replacement-v2" : undefined,
    revision: String(workflowRevision),
    stage: workflowStage,
    scan: duplicateStatusFixture(),
    categories: kinds.map((kind) => {
      const rows = duplicateGeneration
        ? duplicateFixtures.filter((g) =>
            kind === "candidate"
              ? g.kind === "near"
              : kind === "recycle"
                ? g.members.some((m) => targets.has(m.image.id))
                : kind === "retained"
                  ? g.members.some((m) => !targets.has(m.image.id))
                  : workflowKind(g) === kind,
          )
        : [];
      const ids = new Set(
        rows
          .flatMap((g) => g.members.map((m) => m.image.id))
          .filter((id) => !duplicateTrashed.has(id)),
      );
      return {
        kind,
        images:
          kind === "recycle"
            ? targets.size
            : kind === "retained"
              ? new Set(
                  duplicateFixtures
                    .flatMap((g) => g.members.map((m) => m.image.id))
                    .filter(
                      (id) => !duplicateTrashed.has(id) && !targets.has(id),
                    ),
                ).size
              : ids.size,
        groups: rows.length,
        cleanupCount: [...ids].filter((id) => targets.has(id)).length,
      };
    }),
    cleanupCount: targets.size,
    protectedCount: [...reviewChoices.values()].filter((c) => c === "protected")
      .length,
    lastOperationId: lastWorkflowOperation,
    errors: [],
  };
}
function checkWorkflowVersion(request: {
  generation: string;
  revision?: string;
}) {
  if (
    request.generation !== duplicateGeneration ||
    (request.revision && request.revision !== String(workflowRevision))
  )
    throw Error("结果或整理计划已变化，请重新查看后操作。");
}
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
  else if (command === "get_navigation")
    result = { folders: [], userFolders: folders };
  else if (command === "resolve_tag_labels_command")
    result = (args.tags as string[]).map((tagEn) => ({
      tagEn,
      tagZh: tagEn === "landscape" ? "风景" : null,
    }));
  else if (command === "query_gallery")
    result = query(args.query as GalleryQuery);
  else if (command === "query_tag_preview_command")
    result = query({
      scope: "all",
      offset: Number(args.offset ?? 0),
      limit: Number(args.limit ?? 18),
    } as GalleryQuery);
  else if (command === "list_tag_covers_command")
    result = (args.tags as string[]).map((tag, i) => ({
      tag,
      image: images[i % images.length],
      pinned: false,
      x: 50,
      y: 50,
      zoom: 1,
    }));
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
  else if (command === "list_favorite_tags") result = [...favoriteTags];
  else if (command === "set_favorite_tag") {
    if (args.favorite) favoriteTags.add(String(args.tag));
    else favoriteTags.delete(String(args.tag));
    result = [...favoriteTags];
  } else if (command === "undo_viewer_trash") {
    if (Number(args.revision) !== revision)
      throw new Error("图库已有后续修改，请到应用回收站还原这张图片");
    const image = images.find((i) => i.id === args.imageId);
    if (!image?.trashed) throw new Error("图片状态已变化");
    image.trashed = false;
    revision++;
    result = {
      images: [image],
      successfulIds: [image.id],
      failures: [],
      revision,
    };
  } else if (command === "mutate_images") {
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
  } else if (command === "get_user_folder_rule_command") result = null;
  else if (command === "list_jobs") result = records;
  else if (command === "job_history") {
    const q = args.query as { state?: string; cursor?: string; limit?: number };
    const rows = records
      .filter(
        (j) =>
          !taskActive.includes(j.state) &&
          (!q.state || q.state === "all" || j.state === q.state),
      )
      .sort((a, b) => b.updatedAt - a.updatedAt || b.id.localeCompare(a.id));
    const offset = Number(q.cursor ?? 0),
      limit = q.limit ?? 20;
    result = {
      rows: rows.slice(offset, offset + limit),
      ...(offset + limit < rows.length
        ? { nextCursor: String(offset + limit) }
        : {}),
    };
  } else if (command === "clear_job_history") {
    if (!args.confirmed) throw new Error("请确认清除任务记录");
    const before = records.length;
    for (let i = records.length - 1; i >= 0; i--)
      if (!taskActive.includes(records[i].state)) records.splice(i, 1);
    result = before - records.length;
  } else if (command === "retry_job") {
    const old = records.find((j) => j.id === args.id);
    if (!old) throw new Error("任务不存在");
    result = await previewInvoke("start_job", { request: { ...old.request } });
  } else if (command === "duplicate_workflow_status") {
    if (args.retry) {
      snapshotRetried = true;
      snapshotReads = 0;
    }
    const pending = snapshotFixture === "loading" && snapshotReads++ < 2;
    const failed = snapshotFixture === "error" && !snapshotRetried;
    result =
      pending || failed
        ? {
            ...workflowStatusFixture(),
            stage: failed ? "error" : "loading",
            categories: [],
            cleanupCount: 0,
            errors: failed ? ["合成元数据读取失败"] : [],
          }
        : workflowStatusFixture();
  } else if (command === "query_duplicate_workflow") {
    const q = args.query as DuplicateWorkflowQuery;
    checkWorkflowVersion(q);
    const targets = new Set(workflowTargets());
    const category = duplicateFixtures.filter((g) =>
      q.kind === "candidate"
        ? g.kind === "near"
        : q.kind === "recycle"
          ? g.members.some((m) => targets.has(m.image.id))
          : q.kind === "retained"
            ? g.members.some((m) => !targets.has(m.image.id))
            : workflowKind(g) === q.kind,
    );
    const list = !q.groupId || q.reanchor === true;
    const rows = list
      ? category
      : duplicateFixtures.filter((g) => g.id === q.groupId);
    const anchor = q.reanchor
      ? category.findIndex((g) => g.id === q.groupId)
      : -1;
    const offset = q.reanchor ? Math.max(0, anchor) : Number(q.cursor || 0),
      limit = Math.min(q.limit, list ? 20 : 50);
    result = {
      generation: duplicateGeneration,
      revision: String(workflowRevision),
      groups: !list
        ? rows.map((g) => workflowGroup(g, offset, limit))
        : rows
            .slice(offset, offset + limit)
            .map((g) => ({ ...workflowGroup(g), kind: q.kind })),
      totalGroups: rows.length,
      nextCursor:
        list && offset + limit < rows.length ? String(offset + limit) : null,
      paging: list
        ? {
            offset,
            previousCursor: offset > 20 ? String(offset - 20) : null,
            previousLimit: Math.min(offset, 20),
          }
        : undefined,
    };
  } else if (command === "edit_duplicate_review") {
    const request = args.request as DuplicateReviewEdit;
    checkWorkflowVersion(request);
    const group = duplicateFixtures.find((g) => g.id === request.groupId);
    if (!group) throw Error("分组已不存在。");
    const member = group.members.find((m) => m.image.id === request.imageId);
    if (request.action === "confirm") confirmedGroups.add(group.id);
    else {
      if (!member) throw Error("图片不在当前分组。");
      const keeper = reviewKeepers.get(group.id) ?? group.reference.image.id;
      if (request.action === "keeper") {
        reviewKeepers.set(group.id, member.image.id);
        reviewChoices.set(member.image.id, "protected");
      } else if (request.action === "keep")
        reviewChoices.set(member.image.id, "protected");
      else if (request.action === "clean") {
        if (workflowStage !== "refined" && group.kind !== "exact")
          throw Error("请先完成近似复检。");
        if (member.image.id === keeper)
          throw Error("每组至少保留一张图片，请先更换保留基准。");
        reviewChoices.set(member.image.id, "manual");
      }
    }
    ++workflowRevision;
    result = workflowStatusFixture();
  } else if (command === "prepare_duplicate_workflow") {
    const request = args.request as DuplicateWorkflowRequest;
    checkWorkflowVersion(request);
    if (workflowStage !== "refined") throw Error("请先完成两步检索。");
    const id = `preview-workflow-${Date.now()}`,
      targets = workflowTargets();
    const preview: DuplicateActionPreview = {
      operationId: id,
      generation: request.generation,
      groupId: `workflow:${workflowRevision}`,
      action: "trash",
      targetCount: targets.length,
      totalBytes: targets.length * 1000000,
      protectedCount: reviewChoices.size,
      skippedCount: 0,
      metadata: {
        favoriteCount: 0,
        classifiedCount: 0,
        customTaggedCount: 0,
        supplementTaggedCount: 0,
      },
      sample: [],
      warnings: [],
      expiresAt: Date.now() + 60000,
      requiresSystemConfirmation: false,
    };
    workflowOperations.set(id, {
      preview,
      targets,
      status: {
        operationId: id,
        generation: request.generation,
        groupId: preview.groupId,
        action: "trash",
        state: "prepared",
        stage: "planning",
        stageProcessed: 0,
        stageTotal: targets.length,
        processed: 0,
        total: targets.length,
        succeeded: 0,
        failed: 0,
        skipped: 0,
        undoable: false,
        affectedGroupIds: [],
        affectedGroupsTruncated: false,
        errors: [],
        updatedAt: Date.now(),
      },
    });
    result = preview;
  } else if (command === "apply_duplicate_action") {
    const request = args.request as { operationId: string; confirmed: boolean };
    const operation = workflowOperations.get(request.operationId);
    if (!operation) throw Error("整理批次不存在。");
    if (operation.preview.groupId !== `workflow:${workflowRevision}`)
      throw Error("整理计划已变化，请重新操作。");
    for (const id of operation.targets) duplicateTrashed.add(id);
    lastWorkflowOperation = request.operationId;
    ++workflowRevision;
    operation.status = {
      ...operation.status,
      state: "completed",
      stage: "applying",
      stageProcessed: operation.targets.length,
      processed: operation.targets.length,
      succeeded: operation.targets.length,
      undoable: true,
    };
    const job: JobRecord = {
      id: `job-${request.operationId}`,
      request: {
        kind: "duplicateManagement",
        thumbnails: false,
        sourceFolderId: null,
        duplicateOptions: null,
        duplicateOperationId: request.operationId,
      },
      state: "completed",
      stage: "整理完成",
      processed: operation.targets.length,
      total: operation.targets.length,
      failures: 0,
      errors: [],
      updatedAt: Date.now(),
    };
    records.unshift(job);
    result = job;
  } else if (command === "duplicate_operation_status") {
    const operation = workflowOperations.get(String(args.operationId));
    if (!operation) throw Error("整理批次不存在。");
    result = { ...operation.status };
  } else if (command === "duplicate_operation_results")
    result = {
      operationId: (args.query as { operationId: string }).operationId,
      items: [],
      nextCursor: null,
    };
  else if (command === "undo_duplicate_action") {
    const operation = workflowOperations.get(String(args.operationId));
    if (!operation) throw Error("整理批次不存在。");
    for (const id of operation.targets) duplicateTrashed.delete(id);
    operation.status.undoable = false;
    ++workflowRevision;
    const id = `preview-undo-${Date.now()}`;
    const status = {
      ...operation.status,
      operationId: id,
      action: "restore",
      undoable: false,
    };
    workflowOperations.set(id, {
      preview: { ...operation.preview, operationId: id, action: "restore" },
      status,
      targets: [],
    });
    const job: JobRecord = {
      id: `job-${id}`,
      request: {
        kind: "duplicateUndo",
        thumbnails: false,
        sourceFolderId: null,
        duplicateOptions: null,
        duplicateOperationId: id,
      },
      state: "completed",
      stage: "撤销完成",
      processed: operation.targets.length,
      total: operation.targets.length,
      failures: 0,
      errors: [],
      updatedAt: Date.now(),
    };
    records.unshift(job);
    result = job;
  } else if (command === "clear_duplicate_management_records") {
    workflowOperations.clear();
    lastWorkflowOperation = null;
  } else if (command === "duplicate_status") result = duplicateStatusFixture();
  else if (command === "query_duplicate_groups") {
    const q = args.query as DuplicateQuery;
    if (q.generation !== duplicateGeneration)
      throw new Error("预览示例：结果版本已变化，请刷新。");
    const rows = duplicateFixtures.filter(
      (g) => q.kind === "all" || q.kind === g.kind,
    );
    const offset = Number(q.cursor || 0),
      limit = Math.min(q.limit, 20);
    result = {
      generation: duplicateGeneration,
      groups: rows.slice(offset, offset + limit).map((g) => ({
        ...g,
        members: g.members.slice(0, 6),
        nextCursor: g.members.length > 6 ? "6" : null,
      })),
      totalGroups: rows.length,
      nextCursor: offset + limit < rows.length ? String(offset + limit) : null,
    };
  } else if (command === "query_duplicate_members") {
    const q = args.query as DuplicateMemberQuery;
    if (q.generation !== duplicateGeneration)
      throw new Error("预览示例：结果版本已变化，请刷新。");
    const group = duplicateFixtures.find((g) => g.id === q.groupId);
    if (!group) throw new Error("预览示例：分组不存在。");
    const offset = Number(q.cursor || 0),
      limit = Math.min(q.limit, 50);
    result = {
      generation: duplicateGeneration,
      groupId: q.groupId,
      members: group.members.slice(offset, offset + limit),
      totalMembers: group.totalMembers,
      nextCursor:
        offset + limit < group.members.length ? String(offset + limit) : null,
    };
  } else if (command === "clear_duplicate_cache") {
    duplicateGeneration = null;
    duplicateFixtureState = "empty";
    workflowStage = "empty";
    reviewChoices.clear();
    reviewKeepers.clear();
    confirmedGroups.clear();
    ++workflowRevision;
  } else if (command === "start_job") {
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
    if (r.request.kind === "duplicates") {
      const options = r.request.duplicateOptions as DuplicateOptions | null;
      duplicateSensitivity = options?.sensitivity || "standard";
      duplicateVerificationMode = options?.verificationMode || "candidate";
      if (duplicateVerificationMode === "organize") {
        duplicateGeneration ??= `preview-${++duplicateSequence}`;
        workflowStage = "refined";
      } else if (duplicateVerificationMode !== "refine") {
        duplicateGeneration = `preview-${++duplicateSequence}`;
        workflowStage = "scanned";
        reviewChoices.clear();
        reviewKeepers.clear();
        confirmedGroups.clear();
      } else workflowStage = "refined";
      ++workflowRevision;
      duplicateFixtureState = "ready";
      r.state = "completed";
      r.stage = "布局预览示例完成";
      r.processed = r.total = 296;
    }
    taskMeta(r);
    records.unshift(r);
    result = r;
  } else if (command === "control_job") {
    const r = records.find((r) => r.id === args.id);
    if (r)
      r.state =
        args.action === "retrySave"
          ? "completed"
          : args.action === "cancel"
            ? "cancelled"
            : args.action === "pause"
              ? "paused"
              : "running";
    if (r) {
      r.updatedAt = Date.now();
      taskMeta(r);
    }
  } else if (command === "suggest_known_auto_tags_command")
    result = [{ tagEn: "landscape", tagZh: "风景", imageCount: 48 }];
  else if (command === "list_tag_management_state_command")
    result = { folders: [], unclassifiedTags: ["风景", "收藏参考"] };
  else if (command === "list_tag_dictionary_browser_command")
    result = layoutTags
      ? {
          groups: [
            "人物",
            "作品",
            "人数构成",
            "身体部位",
            "头发",
            "五官表情",
            "饰品穿戴",
            "场景",
            "构图",
          ].map((zh, n) => ({ id: `layout-${n}`, zh })),
          tags: [
            "项链",
            "腰领",
            "手镯",
            "眼镜与其他面部饰品",
            "皮带",
            "黑色丝带",
            "红色丝带",
            "黑蝴蝶结",
            "女仆头饰",
            "红色蝴蝶结",
            "从侧面看向远处的长名称",
            "墙壁上的装饰画",
            "银色发饰",
            "复杂的建筑背景",
            "窗边",
            "城市街道",
            "傍晚",
            "风景",
          ].map((tagZh, n) => ({
            tagEn:
              n === 0
                ? "landscape"
                : [
                    "detached_collar",
                    "a_long_untranslated_example_tag_with_several_words",
                    "bracelet",
                    "glasses",
                    "black_ribbon",
                  ][n % 5] + `_${n}`,
            tagZh,
            group: `layout-${n % 9}`,
            imageCount: 669584 - n * 19053,
            sortIndex: n,
          })),
        }
      : {
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
