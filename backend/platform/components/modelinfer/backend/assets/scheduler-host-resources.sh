export LC_ALL=C
cpu_sample() {
    awk '/^cpu / {
        total = 0
        for (i = 2; i <= 9; i++) total += $i
        printf "%.0f %.0f\n", total, $5 + $6
    }' /proc/stat
}
set -- $(cpu_sample)
before_total=${1:-0}
before_idle=${2:-0}
sleep 0.4
awk -v total="$before_total" -v idle="$before_idle" '
    /^cpu[0-9]+ / {cores++}
    /^cpu / {
        now = 0
        for (i = 2; i <= 9; i++) now += $i
        delta = now - total
        unused = $5 + $6 - idle
        if (total > 0 && delta > 0 && unused >= 0 && unused <= delta)
            usage = sprintf("%.2f", (1 - unused / delta) * 100)
    }
    END {if (cores) printf "cpu\t%d\t%s\n", cores, usage}
' /proc/stat
awk '
    /^MemTotal:/ {total = $2}
    /^MemAvailable:/ {available = $2}
    END {if (total > 0) printf "mem\t%d\t%s\n", total, available}
' /proc/meminfo

memory_type=dedicated
jetson_load=
if test -r /proc/device-tree/compatible && tr '\000' ' ' </proc/device-tree/compatible | grep -Eq 'nvidia,tegra'; then
    memory_type=unified
    for file in /sys/devices/platform/17000000.gpu/load /sys/devices/gpu.0/load; do
        if test -r "$file"; then
            jetson_load=$(awk '$1 ~ /^[0-9]+$/ && $1 <= 1000 {printf "%.1f", $1 / 10}' "$file")
            test -z "$jetson_load" || break
        fi
    done
fi
gpu_rows=
if command -v nvidia-smi >/dev/null 2>&1; then
    gpu_rows=$(timeout 3 nvidia-smi --query-gpu=name,utilization.gpu,memory.total,memory.free --format=csv,noheader,nounits 2>/dev/null |
        awk -F, -v kind="$memory_type" -v load="$jetson_load" '
            NF == 4 {
                for (i = 1; i <= NF; i++) gsub(/^ +| +$/, "", $i)
                memory = index(tolower($1), "gb10") ? "unified" : kind
                if ($2 !~ /^[0-9]+$/ && length(load)) $2 = load
                printf "gpu\t%s\t%s\t%s\t%s\t%s\n", $1, $2, $3, $4, memory
            }
        ')
fi
if test -n "$gpu_rows"; then
    printf '%s\n' "$gpu_rows"
elif test -n "$jetson_load"; then
    printf 'gpu\tNVIDIA Jetson\t%s\t\t\tunified\n' "$jetson_load"
fi
