from __future__ import annotations

import json
import math
from typing import cast


type JsonScalar = None | bool | int | float | str
type JsonValue = JsonScalar | list[JsonValue] | dict[str, JsonValue]
type JsonObject = dict[str, JsonValue]


def validate_json(value: object, context: str = "JSON value") -> JsonValue:
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError(f"{context} contains a non-finite number")
        return value
    if isinstance(value, list):
        return [validate_json(item, context) for item in cast(list[object], value)]
    if isinstance(value, dict):
        result: JsonObject = {}
        for key, item in cast(dict[object, object], value).items():
            if not isinstance(key, str):
                raise ValueError(f"{context} contains a non-string object key")
            result[key] = validate_json(item, context)
        return result
    raise ValueError(f"{context} contains unsupported value {type(value).__name__}")


def decode_json(text: str, context: str = "JSON input") -> JsonValue:
    try:
        value: object = json.loads(text)
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid {context}: {error}") from error
    return validate_json(value, context)


def require_object(value: JsonValue, context: str) -> JsonObject:
    if not isinstance(value, dict):
        raise ValueError(f"{context} is not an object")
    return value
