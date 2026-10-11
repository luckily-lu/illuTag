import type { GalleryImage } from "../../shared/contracts/GalleryImage";
import type { GalleryLayoutItem } from "../../types/gallery";
export class MasonryLayout {
  items: GalleryLayoutItem[] = [];
  columns: GalleryLayoutItem[][] = [];
  height = 0;
  private width = 0;
  private source: GalleryImage[] = [];
  update(
    images: GalleryImage[],
    width: number,
    url: (path: string) => string,
    gap = 16,
  ) {
    width = Math.max(1, width);
    const count = Math.max(1, Math.floor((width + gap) / (224 + gap))),
      colWidth = (width - gap * (count - 1)) / count;
    const append =
      width === this.width &&
      images.length >= this.source.length &&
      this.source.every(
        (i, n) =>
          i.id === images[n]?.id &&
          i.width === images[n]?.width &&
          i.height === images[n]?.height,
      );
    if (!append) {
      this.items = [];
      this.columns = Array.from({ length: count }, () => []);
    }
    const heights = this.columns.map((col) =>
      col.length
        ? col[col.length - 1]!.y + col[col.length - 1]!.height + gap
        : 0,
    );
    for (let index = this.items.length; index < images.length; index++) {
      const image = images[index]!,
        columnIndex = heights.indexOf(Math.min(...heights)),
        height =
          image.width > 0 && image.height > 0
            ? (colWidth * image.height) / image.width
            : colWidth;
      const item: GalleryLayoutItem = {
        id: image.id,
        thumbnailUrl: url(image.thumbnailPath || image.path),
        x: columnIndex * (colWidth + gap),
        y: heights[columnIndex]!,
        width: colWidth,
        height,
        columnIndex,
      };
      this.items.push(item);
      this.columns[columnIndex]!.push(item);
      heights[columnIndex]! += height + gap;
    }
    // Metadata changes can change thumbnail URLs without changing geometry.
    this.items.forEach(
      (item, index) =>
        (item.thumbnailUrl = url(
          images[index]!.thumbnailPath || images[index]!.path,
        )),
    );
    this.width = width;
    this.source = images.slice();
    this.height = Math.max(0, Math.max(0, ...heights) - gap);
  }
  visible(top: number, bottom: number) {
    const result: GalleryLayoutItem[] = [];
    for (const col of this.columns) {
      let lo = 0,
        hi = col.length;
      while (lo < hi) {
        const mid = (lo + hi) >>> 1;
        const item = col[mid]!;
        if (item.y + item.height < top) lo = mid + 1;
        else hi = mid;
      }
      for (
        let index = lo;
        index < col.length && col[index]!.y <= bottom;
        index++
      )
        result.push(col[index]!);
    }
    return result;
  }
}
