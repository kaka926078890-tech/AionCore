-- Migration 014: Point FinClaw agent metadata at the FinDesk logo asset.

UPDATE agent_metadata
SET icon = '/api/assets/logos/tools/finclaw.png',
    updated_at = unixepoch('now','subsec')*1000
WHERE id = 'f1c1a000';
