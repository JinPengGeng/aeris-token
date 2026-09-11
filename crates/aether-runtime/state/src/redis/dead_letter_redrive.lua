-- Atomically redrive one DLQ entry and make retries idempotent.
-- KEYS: source DLQ, destination stream, idempotency marker key
-- ARGV: source entry ID, destination MAXLEN (0 = unbounded), field pairs
if #KEYS ~= 3 or #ARGV < 4 or (#ARGV - 2) % 2 ~= 0 then
    return redis.error_reply('ERR invalid dead-letter redrive arguments')
end
if KEYS[1] == KEYS[2] then
    return redis.error_reply('ERR dead-letter redrive source and destination must differ')
end
if type(redis.acl_check_cmd) ~= 'function' then
    return redis.error_reply('ERR dead-letter redrive requires Redis 7 or later for ACL preflight')
end
local entry_id = ARGV[1]
local maxlen = tonumber(ARGV[2])
if maxlen == nil or maxlen < 0 or maxlen ~= math.floor(maxlen) then
    return redis.error_reply('ERR dead-letter redrive maxlen must be a non-negative integer')
end
if not redis.acl_check_cmd('GET', KEYS[3]) then
    return redis.error_reply('NOPERM dead-letter redrive requires GET permission')
end
if not redis.acl_check_cmd('XRANGE', KEYS[1], entry_id, entry_id, 'COUNT', 1) then
    return redis.error_reply('NOPERM dead-letter redrive requires XRANGE permission')
end
local marker = redis.call('GET', KEYS[3])
if marker then
    return {2, marker}
end
local existing = redis.call('XRANGE', KEYS[1], entry_id, entry_id, 'COUNT', 1)
if #existing == 0 then
    return {0, ''}
end
local destination_type = redis.call('TYPE', KEYS[2]).ok
if destination_type ~= 'none' and destination_type ~= 'stream' then
    return redis.error_reply('WRONGTYPE dead-letter redrive destination must be a stream')
end
local append = {'XADD', KEYS[2]}
if maxlen > 0 then
    append = {'XADD', KEYS[2], 'MAXLEN', '~', ARGV[2]}
end
append[#append + 1] = '*'
for index = 3, #ARGV do
    append[#append + 1] = ARGV[index]
end
if not redis.acl_check_cmd(unpack(append)) then
    return redis.error_reply('NOPERM dead-letter redrive requires XADD permission')
end
if not redis.acl_check_cmd('SET', KEYS[3], 'probe') then
    return redis.error_reply('NOPERM dead-letter redrive requires SET permission')
end
if not redis.acl_check_cmd('XDEL', KEYS[1], entry_id) then
    return redis.error_reply('NOPERM dead-letter redrive requires XDEL permission')
end
local destination_id = redis.call(unpack(append))
redis.call('SET', KEYS[3], destination_id)
redis.call('XDEL', KEYS[1], entry_id)
return {1, destination_id}
