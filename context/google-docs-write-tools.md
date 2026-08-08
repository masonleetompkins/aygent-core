# Google Docs/Drive WRITE tools (Mason 08-08)

Mason needs the full write surface for the Google service account — specifically
to WRITE into an existing Doc so he can edit it and Cleo reads the changes back.

## Added (descriptors.rs, google connector) — CODE COMPLETE, integrity-verified
- **google_edit_doc** — documents.batchUpdate on an existing Doc.
  base docs.googleapis.com, path /v1/documents/{document_id}:batchUpdate,
  raw_params ["requests"], body {"requests":{requests}}. Full Docs grammar
  (insertText/replaceAllText/deleteContentRange/format). access=Write, danger=true.
- **google_append_doc** — append text to end via insertText+endOfSegmentLocation.
  raw_params ["text"] (JSON string). Same base/path.
- **google_create_drive_file** — create any mimeType (incl. folders) in a shared
  folder. base "" (www.googleapis.com), path /drive/v3/files. Metadata-only.
- **google_auth.rs SCOPES** += auth/documents (was missing; Docs edit 403s without
  it). Correct + needed.

## VERIFIED (Cleo, 2026-08-08)
- cargo check: clean. cargo test catalog_integrity: 5/5 pass
  (incl. raw_params_are_referenced_by_the_body, mutating_methods_are_marked_write).
- Descriptors are WELL-FORMED and consistent with the working google_edit_
  presentation / google_create_doc patterns.

## THE REAL BLOCKER (found by a LIVE call — the connector-suite lesson again)
Tried google_create_doc(kind=document, parents=["1ZpEw…Cleo Docs folder"]):
  -> 403: "The user's Drive storage quota has been exceeded."
This is NOT a scope or Docs-API problem. It's the SERVICE-ACCOUNT OWNERSHIP/QUOTA
quirk (already in memory from the connector suite): a service account has ~0 Drive
storage, and a file it CREATES is owned by the service account -> hits its 0 quota,
EVEN inside a regular My-Drive folder that a human shared. The share grants edit
rights, not storage ownership transfer.

### Why this matters for the tools
- CREATE tools (google_create_doc, google_create_drive_file, google_create_
  presentation) will 403-quota when the target is a MY-DRIVE folder.
- EDIT tools (google_edit_doc, google_append_doc, google_edit_presentation,
  google_write_sheet) DON'T create a file -> they should NOT hit quota. They edit
  a doc a HUMAN already created + owns. THIS is the loop Mason actually wants
  ("I write into a doc you can edit" == edit an existing human-owned doc).

### The fix (two paths; recommend BOTH)
1. **Make the write→edit→read loop work on a HUMAN-CREATED doc** (no create needed):
   Mason creates a Doc in the shared "Cleo Docs" folder (id
   1ZpEwasKohJC2KYgCBfF0xGKOl4_g4XJT), shares edit access with the service account,
   then google_append_doc / google_edit_doc write into it. NO quota issue because
   the human owns the file. This is the primary, working path once the new tools
   are in a build.
2. **For CREATE to work**, the shared folder must live on a **Shared Drive**
   (formerly Team Drive), not My Drive — files on a Shared Drive are owned by the
   drive, not the account, so no per-account quota. (Alternatively: create with
   `supportsAllDrives=true` into a Shared Drive folder.) A regular shared My-Drive
   folder can NEVER host a service-account-created file.

### Honest error message TODO (nice-to-have, not blocking)
connector_exec's 403 handler should special-case "storage quota has been
exceeded" -> tell the user plainly: "A service account can't OWN new Drive files.
Either edit an existing doc you created + shared, or put the folder on a Shared
Drive." (Same spirit as the connector-suite "good errors name the fix".)

## STATUS
- Tools + scope: DONE, correct, tested well-formed. Ready to commit to staging.
- Live create verified BLOCKED by the quota quirk (expected for a My-Drive folder).
- Live edit of a human-owned doc: NOT yet testable (the new edit tools aren't in a
  running build yet, and the folder had no human-owned doc to edit). Verify after
  next bundle: Mason drops a Doc into Cleo Docs, then google_append_doc into it.
