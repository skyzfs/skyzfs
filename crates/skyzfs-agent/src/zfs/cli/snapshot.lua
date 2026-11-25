-- skyzfs-agent: take @t of a pool and of every descendant in one
-- transaction group, and read the manifest's content in it (the agent's
-- crate, zfs/cli.rs). Run by `zfs program -j POOL - POOL T PROPERTY`.
-- Nothing is taken unless every member's @t can be: each is checked,
-- and every property read, before the first is taken.
local argv = (...)["argv"]
local pool, t, assist = argv[1], argv[2], argv[3]

local names = {}
local function walk(ds)
    names[#names + 1] = ds
    for child in zfs.list.children(ds) do
        walk(child)
    end
end
walk(pool)

for _, ds in ipairs(names) do
    local err = zfs.check.snapshot(ds .. "@" .. t)
    if err ~= 0 then
        return {failed = ds .. "@" .. t, errno = tostring(err)}
    end
end

-- A list as an iterator, or an iterator as it is.
local function each(list)
    if type(list) == "table" then
        local i = 0
        return function()
            i = i + 1
            return list[i]
        end
    end
    return list
end

local user_properties = zfs.list.user_properties or zfs.list.properties
local members = {}
for _, ds in ipairs(names) do
    local set = {}
    for property in each(zfs.list.system_properties(ds)) do
        local ok, value, source = pcall(zfs.get_prop, ds, property)
        if not ok then
            return {failed = ds, property = property, why = tostring(value)}
        end
        if source == ds and value ~= nil then
            set[property] = tostring(value)
        end
    end
    for property, value, source in user_properties(ds) do
        if source == ds and property ~= assist then
            set[property] = tostring(value)
        end
    end
    members[ds] = {
        type = tostring(zfs.get_prop(ds, "type")),
        guid = tostring(zfs.get_prop(ds, "guid")),
        ["local"] = set,
    }
end

for i, ds in ipairs(names) do
    local err = zfs.sync.snapshot(ds .. "@" .. t)
    if err ~= 0 then
        return {failed = ds .. "@" .. t, errno = tostring(err), taken = tostring(i - 1)}
    end
end

for _, ds in ipairs(names) do
    members[ds]["snapshot_guid"] = tostring(zfs.get_prop(ds .. "@" .. t, "guid"))
end

return {
    createtxg = tostring(zfs.get_prop(pool .. "@" .. t, "createtxg")),
    members = members,
}
