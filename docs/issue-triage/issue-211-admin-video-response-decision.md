# Issue #211 admin video response decision

Admin video task detail responses intentionally omit `original_request_body` and
`request_metadata`. These private persistence/runtime fields can contain provider authorization
headers, signed URLs, cookies, and request prompts. Workers still read them from the private
persistence path when reconstructing a task; the admin API returns `null` so a compromised
session, proxy log, or browser console cannot exfiltrate the values.

The list endpoint already uses a summary projection that excludes these fields. This change
closes the detail endpoint boundary and keeps the existing error projection (error codes only)
for the same reason. Existing persisted records are not modified by this response-only change.
