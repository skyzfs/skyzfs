-- skyzfs-agent: take @t of a pool and of every descendant in one
-- transaction group, and read the manifest's content in it (the agent's
-- crate, zfs/cli.rs). Run by `zfs program -j POOL - POOL T PROPERTY`.
-- Nothing is taken unless every member's @t can be: each is checked,
-- and every property read, before the first is taken. The sandbox has
-- no pcall: a property that cannot be read raises an error, which ends
-- the program there, before anything is taken.
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

-- A value as the manifest carries it: printable ASCII as it is, any
-- other as its bytes in lower-case hex under its encoding. `zfs program
-- -j` writes a string through the C library's multibyte conversion,
-- which stops part way at a byte it cannot convert and still exits 0;
-- here, after the take, that would lose the manifest of snapshots taken.
local function value_of(v)
    v = tostring(v)
    if string.find(v, "[^\32-\126]") == nil then
        return v
    end
    local hex = string.gsub(v, ".", function(c)
        return string.format("%02x", string.byte(c))
    end)
    return {encoding = "hex", value = hex}
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
        local value, source = zfs.get_prop(ds, property)
        if source == ds and value ~= nil then
            set[property] = value_of(value)
        end
    end
    for property, value, source in user_properties(ds) do
        if source == ds and property ~= assist then
            set[property] = value_of(value)
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
