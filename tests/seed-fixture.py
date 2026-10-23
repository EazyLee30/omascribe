#!/usr/bin/env python3
"""Add clearly fictional content to an initialized, isolated test workspace."""

import datetime, json, pathlib, sqlite3, sys, uuid

root = pathlib.Path(sys.argv[1]).resolve()
if root.name == "omascribe" or not str(root).startswith("/tmp/"):
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
conn.commit()
print(mid)
