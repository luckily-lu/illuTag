import argparse
import json
import os
import sys
import time

try:
    import numpy as np
except ModuleNotFoundError as e:
    missing = getattr(e, "name", "unknown")
    print(
        f"Missing Python module: {missing}. Install with: pip install onnxruntime numpy pillow",
        file=sys.stderr,
    )
    sys.exit(2)

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ort_runtime import create_session
from wd_tagger_service import (
    infer_layout,
    load_tagger_config,
    load_tags,
    preprocess_image,
    resolve_hw,
    to_probs,
)


def parse_args():
    parser = argparse.ArgumentParser(description="Local ONNX tagger single-image test")
    parser.add_argument("--image", required=True, help="Source image path")
    parser.add_argument("--model", required=True, help="model.onnx path")
    parser.add_argument("--tags", required=True, help="selected_tags.csv path")
    parser.add_argument("--general-threshold", type=float, default=0.2)
    parser.add_argument("--character-threshold", type=float, default=0.2)
    parser.add_argument("--image-id", default="")
    parser.add_argument("--provider", choices=["cpu", "cuda"], default="cpu")
    return parser.parse_args()


def main():
    args = parse_args()
    if not os.path.isfile(args.image):
        raise RuntimeError(f"Image not found: {args.image}")
    if not os.path.isfile(args.model):
        raise RuntimeError(f"Model not found: {args.model}")
    if not os.path.isfile(args.tags):
        raise RuntimeError(f"selected_tags.csv not found: {args.tags}")

    config = load_tagger_config(args.model)
    preprocess = str(config.get("preprocess", "wd")).strip().lower() or "wd"
    apply_sigmoid = bool(config.get("apply_sigmoid", False))
    configured_output = str(config.get("output_name", "") or "").strip()
    if config.get("general_threshold") is not None:
        general_threshold = float(config["general_threshold"])
    else:
        general_threshold = float(args.general_threshold)
    if config.get("character_threshold") is not None:
        character_threshold = float(config["character_threshold"])
    else:
        character_threshold = float(args.character_threshold)

    session = create_session(args.model, args.provider, "wd-test")
    input_info = session.get_inputs()[0]
    outputs = session.get_outputs()
    input_name = input_info.name
    output_names = [item.name for item in outputs]
    output_name = configured_output if configured_output in output_names else output_names[0]
    shape = input_info.shape
    layout = infer_layout(shape, preprocess, str(config.get("layout", "") or "").strip().lower())
    input_height, input_width = resolve_hw(shape, layout)

    tensor = preprocess_image(args.image, preprocess, input_height, input_width)
    start = time.perf_counter()
    raw = session.run([output_name], {input_name: tensor})
    elapsed_ms = (time.perf_counter() - start) * 1000.0

    probs = to_probs(raw[0][0], apply_sigmoid)
    tags = load_tags(args.tags)
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

    result = {
        "imageId": args.image_id,
        "ratings": ratings,
        "generalTags": general_tags,
        "characterTags": character_tags,
        "generalThreshold": float(general_threshold),
        "characterThreshold": float(character_threshold),
        "elapsedMs": float(elapsed_ms),
    }
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(str(e), file=sys.stderr)
        sys.exit(1)
