import argparse
import csv
import json
import os
import sys
import time

try:
    import numpy as np
    from PIL import Image
except ModuleNotFoundError as e:
    missing = getattr(e, "name", "unknown")
    print(
        json.dumps(
            {
                "error": f"Missing Python module: {missing}. Install with: pip install onnxruntime numpy pillow"
            },
            ensure_ascii=False,
        ),
        flush=True,
    )
    sys.exit(2)

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ort_runtime import create_session

IMAGENET_MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
IMAGENET_STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)


def parse_args():
    parser = argparse.ArgumentParser(description="DanbooruTagQuery ONNX tagger service")
    parser.add_argument("--model", required=True, help="model.onnx path")
    parser.add_argument("--tags", required=True, help="selected_tags.csv path")
    parser.add_argument("--provider", choices=["cpu", "cuda"], default="cpu")
    return parser.parse_args()


def load_tagger_config(model_path):
    cfg_path = os.path.join(os.path.dirname(os.path.abspath(model_path)), "tagger_config.json")
    config = {
        "preprocess": "dtq",
        "apply_sigmoid": True,
        "output_name": "",
        "layout": "nchw",
        "general_threshold": 0.2,
        "character_threshold": 0.2,
    }
    if os.path.isfile(cfg_path):
        with open(cfg_path, "r", encoding="utf-8") as f:
            loaded = json.load(f)
        if isinstance(loaded, dict):
            config.update(loaded)
    return config


def load_tags(csv_path):
    tags = []
    with open(csv_path, "r", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        for row in reader:
            name = row.get("name", "").strip()
            category_raw = row.get("category", "0").strip()
            if not name:
                continue
            try:
                category = int(category_raw)
            except ValueError:
                category = 0
            tags.append((name, category))
    if not tags:
        raise RuntimeError("No tags loaded from selected_tags.csv")
    return tags


def letterbox_square(image_rgb, size, fill=(0, 0, 0)):
    w, h = image_rgb.size
    scale = size / max(w, h)
    new_w = max(1, int(round(w * scale)))
    new_h = max(1, int(round(h * scale)))
    image = image_rgb.resize((new_w, new_h), Image.Resampling.BILINEAR)
    canvas = Image.new("RGB", (size, size), fill)
    canvas.paste(image, ((size - new_w) // 2, (size - new_h) // 2))
    return canvas


def preprocess_image(image_path, _preprocess, input_height, input_width):
    size = max(input_height, input_width)
    image = Image.open(image_path).convert("RGB")
    image = letterbox_square(image, size, (0, 0, 0))
    if image.size != (input_width, input_height):
        image = image.resize((input_width, input_height), Image.Resampling.BILINEAR)
    arr = np.asarray(image, dtype=np.float32) / 255.0
    arr = (arr - IMAGENET_MEAN) / IMAGENET_STD
    return np.expand_dims(np.transpose(arr, (2, 0, 1)), axis=0)


def resolve_hw(shape, layout):
    if layout != "nhwc":
        height = int(shape[2]) if len(shape) > 2 and isinstance(shape[2], int) else 448
        width = int(shape[3]) if len(shape) > 3 and isinstance(shape[3], int) else 448
    else:
        height = int(shape[1]) if len(shape) > 1 and isinstance(shape[1], int) else 448
        width = int(shape[2]) if len(shape) > 2 and isinstance(shape[2], int) else 448
    return height, width


def infer_layout(shape, _preprocess, configured):
    if configured == "nhwc":
        return "nhwc"
    if len(shape) == 4 and isinstance(shape[1], int) and shape[1] not in (1, 3):
        return "nhwc"
    return "nchw"


def to_probs(raw, apply_sigmoid):
    values = np.asarray(raw, dtype=np.float32).reshape(-1)
    if apply_sigmoid:
        values = 1.0 / (1.0 + np.exp(-np.clip(values, -80.0, 80.0)))
    return values.tolist()


def main():
    args = parse_args()
    if not os.path.isfile(args.model):
        raise RuntimeError(f"Model not found: {args.model}")
    if not os.path.isfile(args.tags):
        raise RuntimeError(f"selected_tags.csv not found: {args.tags}")

    config = load_tagger_config(args.model)
    preprocess = str(config.get("preprocess", "dtq")).strip().lower() or "dtq"
    apply_sigmoid = bool(config.get("apply_sigmoid", True))
    configured_output = str(config.get("output_name", "") or "").strip()
    tags = load_tags(args.tags)
    session = create_session(args.model, args.provider, "wd-service")
    input_info = session.get_inputs()[0]
    outputs = session.get_outputs()
    input_name = input_info.name
    output_names = [item.name for item in outputs]
    if configured_output and configured_output in output_names:
        output_name = configured_output
    else:
        output_name = output_names[0]
    shape = input_info.shape
    layout = infer_layout(shape, preprocess, str(config.get("layout", "") or "").strip().lower())
    input_height, input_width = resolve_hw(shape, layout)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        total_start = time.perf_counter()
        image_id = ""
        image_path = ""
        preprocess_ms = 0.0
        inference_ms = 0.0
        postprocess_ms = 0.0
        total_ms = 0.0
        try:
            request = json.loads(line)
            image_id = str(request.get("image_id", "")).strip()
            image_path = str(request.get("image_path", "")).strip()
            if config.get("general_threshold") is not None:
                general_threshold = float(config["general_threshold"])
            else:
                general_threshold = float(request.get("general_threshold", 0.2))
            if config.get("character_threshold") is not None:
                character_threshold = float(config["character_threshold"])
            else:
                character_threshold = float(request.get("character_threshold", 0.2))

            if not image_path:
                raise RuntimeError("image_path is empty")
            if not os.path.isfile(image_path):
                raise RuntimeError(f"Image not found: {image_path}")

            p0 = time.perf_counter()
            tensor = preprocess_image(image_path, preprocess, input_height, input_width)
            preprocess_ms = (time.perf_counter() - p0) * 1000.0

            i0 = time.perf_counter()
            outputs_raw = session.run([output_name], {input_name: tensor})
            inference_ms = (time.perf_counter() - i0) * 1000.0

            s0 = time.perf_counter()
            probs = to_probs(outputs_raw[0][0], apply_sigmoid)
            if len(probs) != len(tags):
                raise RuntimeError(
                    f"Output length mismatch: got {len(probs)} probs, expected {len(tags)} tags"
                )

            ratings = []
            general_tags = []
            character_tags = []
            for score, (name, category) in zip(probs, tags):
                item = {"tag": name, "score": float(score)}
                if category == 9:
                    ratings.append(item)
                elif category == 0 and score >= general_threshold:
                    general_tags.append(item)
                elif category == 4 and score >= character_threshold:
                    character_tags.append(item)

            ratings.sort(key=lambda x: x["score"], reverse=True)
            general_tags.sort(key=lambda x: x["score"], reverse=True)
            character_tags.sort(key=lambda x: x["score"], reverse=True)
            postprocess_ms = (time.perf_counter() - s0) * 1000.0
            total_ms = (time.perf_counter() - total_start) * 1000.0

            print(
                f"[wd-service] image_id={image_id or image_path} preprocess_ms={preprocess_ms:.2f} "
                f"inference_ms={inference_ms:.2f} postprocess_ms={postprocess_ms:.2f} total_ms={total_ms:.2f}",
                file=sys.stderr,
                flush=True,
            )

            print(
                json.dumps(
                    {
                        "imageId": image_id,
                        "ratings": ratings,
                        "generalTags": general_tags,
                        "characterTags": character_tags,
                        "generalThreshold": general_threshold,
                        "characterThreshold": character_threshold,
                        "elapsedMs": inference_ms,
                        "preprocessMs": preprocess_ms,
                        "inferenceMs": inference_ms,
                        "postprocessMs": postprocess_ms,
                        "totalMs": total_ms,
                    },
                    ensure_ascii=False,
                ),
                flush=True,
            )
        except Exception as error:
            total_ms = (time.perf_counter() - total_start) * 1000.0
            print(
                f"[wd-service] image_id={image_id or image_path or 'unknown'} error={error} total_ms={total_ms:.2f}",
                file=sys.stderr,
                flush=True,
            )
            print(
                json.dumps(
                    {"error": str(error), "imageId": image_id, "totalMs": total_ms},
                    ensure_ascii=False,
                ),
                flush=True,
            )


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(json.dumps({"error": str(e)}, ensure_ascii=False), flush=True)
        sys.exit(1)
