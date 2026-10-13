#!/usr/bin/env python3
"""Build slice.jsonl — the M4 vertical-slice scenario (app Task 8).

Derived from the captured tool-use.jsonl (same envelope shapes, session,
and tool_use ids) by rewriting the conversation around the CAD flow the
slice drives: the agent thinks, writes model.py, runs it (the fake's
`#copy-outputs` directive lands the golden outputs mid-turn, where the
real child's python run would), then reports. Reruns are byte-identical
(fixed uuids, fixed fragmentation).
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SID = "bdd2e831-7c7c-422f-b50a-fc22b182bae3"
WRITE_ID = "toolu_013uZKwucpApefxH7dUCMw1L"
BASH_ID = "toolu_01PiSki6C6xfd2exZn7nabus"
MSG1 = "msg_011CfwREpWzVSGbrJUZ2daL6"
MSG2 = "msg_011CfwREzZqVSGbrJUZ2daL6"

SOURCE = [json.loads(l) for l in (HERE / "tool-use.jsonl").read_text().splitlines()]


class Uuids:
    """Deterministic uuids so reruns are byte-identical."""

    def __init__(self):
        self.n = 0

    def next(self):
        self.n += 1
        return f"00000000-0000-4000-8000-{self.n:012d}"


UUIDS = Uuids()


def event(inner):
    return {
        "type": "stream_event",
        "event": inner,
        "session_id": SID,
        "parent_tool_use_id": None,
        "uuid": UUIDS.next(),
        "api_message_id": MSG1,
    }


def block_start(index, block):
    return event({"type": "content_block_start", "index": index, "content_block": block})


def block_stop(index):
    return event({"type": "content_block_stop", "index": index})


def delta(index, kind, **fields):
    return event(
        {"type": "content_block_delta", "index": index, "delta": {"type": kind, **fields}}
    )


def thinking_tokens(total, step):
    return {
        "type": "system",
        "subtype": "thinking_tokens",
        "estimated_tokens": total,
        "estimated_tokens_delta": step,
        "session_id": SID,
        "uuid": UUIDS.next(),
    }


def tool_result(tool_use_id, content):
    return {
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                {"tool_use_id": tool_use_id, "type": "tool_result", "content": content}
            ],
        },
        "parent_tool_use_id": None,
        "session_id": SID,
        "uuid": UUIDS.next(),
    }


def assistant(content, msg_id):
    return {
        "type": "assistant",
        "message": {
            "model": "claude-opus-5-5",
            "id": msg_id,
            "type": "message",
            "role": "assistant",
            "content": content,
            "container": None,
            "stop_reason": None,
            "stop_sequence": None,
            "stop_details": None,
            "usage": {"input_tokens": 2, "output_tokens": 40},
        },
        "parent_tool_use_id": None,
        "session_id": SID,
        "uuid": UUIDS.next(),
        "timestamp": "2026-10-13T00:00:00.000Z",
        "request_id": "req_slice",
    }


def tool_input_deltas(index, obj):
    """input_json_delta lines whose partial_json concatenates to the full
    input object: a per-key fragmentation of the serialized form."""
    full = json.dumps(obj)
    keys = [f"{json.dumps(k)}: " for k in obj]
    splits = ["{"]
    for at, key in enumerate(keys):
        value = json.dumps(list(obj.values())[at])
        splits.append(key + value)
        if at < len(keys) - 1:
            splits.append(", ")
    splits.append("}")
    assert "".join(splits) == full, "fragmentation must cover the input"
    return [delta(index, "input_json_delta", partial_json=p) for p in splits]


def main():
    init = dict(SOURCE[0])  # the captured system/init line, verbatim shape
    lines = []

    # message 1: thinking, then the Write and Bash tool blocks
    lines.append(init)
    lines.append(event({"type": "message_start", "message": {"id": MSG1}}))
    lines.append(block_start(0, {"type": "thinking", "thinking": "", "signature": ""}))
    lines.append(thinking_tokens(50, 50))
    lines.append(delta(0, "thinking_delta", thinking="box with a lid, two parts"))
    lines.append(thinking_tokens(192, 142))
    lines.append(delta(0, "signature_delta", signature="slice"))
    lines.append(assistant([{"type": "thinking", "thinking": "", "signature": "slice"}], MSG1))
    lines.append(block_stop(0))

    model_py = (
        '"""\nbox with a lid, two parts\n"""\n'
        "WALL, HEIGHT, LID_H = 2.0, 8.0, 10.0\n"
        "# build123d builds base + lid, exports the outputs\n"
    )
    lines.append(
        block_start(
            1,
            {
                "type": "tool_use",
                "id": WRITE_ID,
                "name": "Write",
                "input": {},
                "caller": {"type": "direct"},
            },
        )
    )
    lines += tool_input_deltas(1, {"file_path": "model.py", "content": model_py})
    lines.append(
        assistant(
            [
                {
                    "type": "tool_use",
                    "id": WRITE_ID,
                    "name": "Write",
                    "input": {"file_path": "model.py", "content": model_py},
                    "caller": {"type": "direct"},
                }
            ],
            MSG1,
        )
    )
    lines.append(block_stop(1))
    lines.append(
        tool_result(WRITE_ID, "File created successfully at: model.py (9 lines written)")
    )

    lines.append(
        block_start(
            2,
            {
                "type": "tool_use",
                "id": BASH_ID,
                "name": "Bash",
                "input": {},
                "caller": {"type": "direct"},
            },
        )
    )
    lines += tool_input_deltas(
        2,
        {"command": "python model.py", "description": "generate the model outputs"},
    )
    lines.append(
        assistant(
            [
                {
                    "type": "tool_use",
                    "id": BASH_ID,
                    "name": "Bash",
                    "input": {
                        "command": "python model.py",
                        "description": "generate the model outputs",
                    },
                    "caller": {"type": "direct"},
                }
            ],
            MSG1,
        )
    )
    lines.append(block_stop(2))
    # the fake lands the golden outputs here — where the real child's python
    # run would have written them, mid-turn
    lines.append("#copy-outputs")
    lines.append(
        tool_result(
            BASH_ID,
            "wrote output/model.3mf, output/model.stl, output/model.step, "
            "output/plate.3mf, output/schematic.svg, output/schematic-2.svg, "
            "output/schematic.png, output/schematic.pdf, output/manifest.json",
        )
    )
    lines.append(event({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}))
    lines.append(event({"type": "message_stop"}))

    # message 2: the report the app confirms as the turn's text
    final = (
        "I wrote `model.py` and ran it: the box (base + lid, 2 parts), the "
        "print plate, and both drawing pages are in `output/`."
    )
    lines.append(event({"type": "message_start", "message": {"id": MSG2}}))
    lines.append(block_start(3, {"type": "text", "text": ""}))
    for fragment in [
        "I wrote `model.py` ",
        "and ran it: the box ",
        "(base + lid, 2 parts), ",
        "the print plate, and both ",
        "drawing pages are in `output/`.",
    ]:
        lines.append(delta(3, "text_delta", text=fragment))
    lines.append(assistant([{"type": "text", "text": final}], MSG2))
    lines.append(block_stop(3))
    lines.append(event({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}))
    lines.append(event({"type": "message_stop"}))

    result = dict(SOURCE[-1])  # the captured result line, same shape
    result["result"] = final
    result["session_id"] = SID
    lines.append(result)

    out = HERE / "slice.jsonl"
    out.write_text(
        "".join(
            (l if isinstance(l, str) else json.dumps(l, separators=(",", ":"))) + "\n"
            for l in lines
        )
    )
    print(f"{out}: {len(lines)} lines")


if __name__ == "__main__":
    main()
