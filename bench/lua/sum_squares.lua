-- same program as ../sum_squares.kpls: n from stdin (0..1000), 5000 reps
local n = tonumber(io.read("l"))
local total = 0
for rep = 1, 5000 do
  total = 0
  for i = 1, n do
    total = total + i * i
  end
end
print(total)
