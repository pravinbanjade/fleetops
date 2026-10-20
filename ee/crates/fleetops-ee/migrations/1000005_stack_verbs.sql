-- Stacks gained Create and Reveal (stacks spec §6). Custom roles keep what
-- they could do: Operate on stacks now needs Create to author a stack, and
-- a role that reveals container env may reveal the stack variables behind it.
UPDATE ee_roles SET permissions = json_insert(permissions, '$[#]', 'stacks.create')
WHERE EXISTS (SELECT 1 FROM json_each(ee_roles.permissions) WHERE value = 'stacks.operate')
  AND NOT EXISTS (SELECT 1 FROM json_each(ee_roles.permissions) WHERE value = 'stacks.create');

UPDATE ee_roles SET permissions = json_insert(permissions, '$[#]', 'stacks.reveal')
WHERE EXISTS (SELECT 1 FROM json_each(ee_roles.permissions) WHERE value = 'containers.reveal')
  AND NOT EXISTS (SELECT 1 FROM json_each(ee_roles.permissions) WHERE value = 'stacks.reveal');
