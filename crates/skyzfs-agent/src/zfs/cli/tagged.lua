-- skyzfs-agent: the names t of every snapshot in the pool, of any
-- dataset, that has the hold TAG (the agent's crate, zfs/cli.rs). Read
-- only: `zfs program -n -j POOL - POOL TAG`.
local argv = (...)["argv"]
local pool, tag = argv[1], argv[2]

local tagged = {}
local function walk(ds)
    for snapshot in zfs.list.snapshots(ds) do
        for hold in zfs.list.holds(snapshot) do
            if hold == tag then
                local at = string.find(snapshot, "@", 1, true)
                tagged[string.sub(snapshot, at + 1)] = "1"
            end
        end
    end
    for child in zfs.list.children(ds) do
        walk(child)
    end
end
walk(pool)
return {tagged = tagged}
