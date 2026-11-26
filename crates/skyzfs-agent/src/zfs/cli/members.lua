-- skyzfs-agent: each member's @t that exists, with its GUID (the agent's
-- crate, zfs/cli.rs). Read only: `zfs program -n -j POOL - POOL T`.
local argv = (...)["argv"]
local pool, t = argv[1], argv[2]

if not zfs.exists(pool .. "@" .. t) then
    return {absent = "1"}
end

local members = {}
local function walk(ds)
    if zfs.exists(ds .. "@" .. t) then
        members[ds] = tostring(zfs.get_prop(ds .. "@" .. t, "guid"))
    end
    for child in zfs.list.children(ds) do
        walk(child)
    end
end
walk(pool)
return {members = members}
