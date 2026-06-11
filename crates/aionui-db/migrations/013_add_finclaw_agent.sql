-- Migration 013: Register FinClaw as a first-class local agent (FinDesk).

INSERT INTO agent_metadata
    (id, icon, name, backend, agent_type, agent_source, agent_source_info,
     enabled, command, args, env, native_skills_dirs, behavior_policy, yolo_id,
     sort_order, created_at, updated_at)
VALUES
    ('f1c1a000', '/api/assets/logos/brand/aion.svg', 'FinClaw',
     'finclaw', 'finclaw', 'builtin', '{"binary_name":"finclaw"}',
     1, 'finclaw', '[]', '[]',
     '[".finclaw/skills"]',
     '{"supports_side_question":false}',
     'supervised', 120,
     unixepoch('now','subsec')*1000, unixepoch('now','subsec')*1000)
ON CONFLICT(id) DO UPDATE SET
    icon = excluded.icon,
    name = excluded.name,
    backend = excluded.backend,
    agent_type = excluded.agent_type,
    agent_source = excluded.agent_source,
    agent_source_info = excluded.agent_source_info,
    enabled = excluded.enabled,
    command = excluded.command,
    args = excluded.args,
    env = excluded.env,
    native_skills_dirs = excluded.native_skills_dirs,
    behavior_policy = excluded.behavior_policy,
    yolo_id = excluded.yolo_id,
    sort_order = excluded.sort_order,
    updated_at = unixepoch('now','subsec')*1000;
