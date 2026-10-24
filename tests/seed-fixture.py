#!/usr/bin/env python3
"""Add clearly fictional content to an initialized, isolated test workspace."""

import datetime, json, pathlib, sqlite3, sys, uuid

root = pathlib.Path(sys.argv[1]).resolve()
if root.name == "omascribe" or not str(root).startswith(("/tmp/", "/private/tmp/")):
    raise SystemExit("Fixture seeding is restricted to a /tmp test workspace.")
fixture = json.loads(
    (pathlib.Path(__file__).parent / "fixtures/meeting.json").read_text()
)
conn = sqlite3.connect(root / "meetings.sqlite3")
mid = str(uuid.uuid4())
now = datetime.datetime.now(datetime.timezone.utc).isoformat()
meeting = dict(
    id=mid,
    title=fixture["title"],
    created_at=now,
    status="ready",
    duration_ms=36000,
    run=1,
    notes_cursor=0,
    auto_notes=False,
    notes_fingerprint="",
)
conn.execute("INSERT INTO meetings VALUES (?,?)", (mid, json.dumps(meeting)))
nid = str(uuid.uuid4())
note = dict(
    id=nid,
    meeting_id=mid,
    content=fixture["document"],
    kind="document",
    anchor_ms=0,
    updated_at=now,
)
conn.execute("INSERT INTO notes VALUES (?,?,?)", (nid, mid, json.dumps(note)))
segments = []
for i, text in enumerate(fixture["transcript"]):
    sid = str(uuid.uuid4())
    segment = dict(
        id=sid,
        meeting_id=mid,
        run=1,
        seq=i + 1,
        start_ms=i * 12000,
        end_ms=(i + 1) * 12000,
        text=text,
        revision=1,
    )
    conn.execute(
        "INSERT INTO segments VALUES (?,?,?,?,?)",
        (sid, mid, 1, i + 1, json.dumps(segment)),
    )
    segments.append(segment)
# One committed document section with real citations, so layout and
# delete/restore checks run without waiting for the local writer model.
# Opt-in only: native-ui.js must generate from a clean fixture, because a
# pre-seeded section biases the writer and masks its commit status.
if "--with-document" in sys.argv:
    suggestion = dict(
        id=str(uuid.uuid4()),
        meeting_id=mid,
        kind="section",
        focus="原型评审与测试安排",
        text=(
            "张老师将在周五提交测试结果，预算问题留到下周再评估。"
            "原型已可评审，离线转写安排在周五验证。"
        ),
        sources=[
            dict(
                segment_id=s["id"],
                revision=1,
                start_ms=s["start_ms"],
                end_ms=s["end_ms"],
                quote=s["text"],
                speaker_name=None,
            )
            for s in segments[:2]
        ],
        status="pending",
        stale=False,
    )
    conn.execute(
        "INSERT INTO suggestions VALUES (?,?,?)",
        (suggestion["id"], mid, json.dumps(suggestion, ensure_ascii=False)),
    )
conn.commit()
print(mid)
