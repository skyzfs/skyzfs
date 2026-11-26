-- skyzfs-agent: each member's @t that exists, with its GUID (the agent's
-- crate, zfs/cli.rs). Read only: `zfs program -n -j POOL - POOL T`.
--
-- With a third argument `full` (`zfs program -n -j POOL - POOL T full
-- PROPERTY`), what the manifest needs, of every file system and volume
-- of the pool whether its @t exists or not: its type, GUID, createtxg and
-- locally set properties (but PROPERTY, the agent's own), and, where its
-- @t exists, that snapshot's GUID and createtxg. The properties are the
-- dataset's as they are now: ZFS keeps on a snapshot only the properties
-- set on the snapshot itself, and reads any other through its dataset.
-- The sandbox has no pcall: a property that cannot be read raises an
-- error, which ends the program with nothing returned.
local argv = (...)["argv"]
local pool, t, mode, assist = argv[1], argv[2], argv[3], argv[4]

if mode ~= "full" then
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
end

-- A value as the manifest carries it: printable ASCII as it is, any
-- other as its bytes in lower-case hex under its encoding. `zfs program
-- -j` writes a string through the C library's multibyte conversion,
-- which stops part way at a byte it cannot convert and still exits 0.
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
local datasets = {}
local function walk(ds)
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
    local entry = {
        type = tostring(zfs.get_prop(ds, "type")),
        guid = tostring(zfs.get_prop(ds, "guid")),
        createtxg = tostring(zfs.get_prop(ds, "createtxg")),
        ["local"] = set,
    }
    local at = ds .. "@" .. t
    if zfs.exists(at) then
        entry["snapshot_guid"] = tostring(zfs.get_prop(at, "guid"))
        entry["snapshot_createtxg"] = tostring(zfs.get_prop(at, "createtxg"))
    end
    datasets[ds] = entry
    for child in zfs.list.children(ds) do
        walk(child)
    end
end
walk(pool)
return {datasets = datasets}
