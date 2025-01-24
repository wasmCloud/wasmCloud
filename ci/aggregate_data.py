#!/usr/bin/env python3
import sys
import json

def create_metric_aggregator(m_type, m_contains):
    """
    Create a structure to hold partial sums/counters and eventually produce final merged values for
    one metric. We need to do this because some of the metrics need to be averaged at the end of
    aggregating all the data. Doing this with p90 values and what not isn't technically accurate,
    but we should only average once at the end. Later down the line we might just want to integrate
    aggregating the raw data
    """
    return {
        "type": m_type,
        "contains": m_contains,
        # We'll accumulate data in these fields:
        "count_sum": 0,    # for count/passes/fails
        "has_count": False,
        "passes_sum": 0,
        "has_passes": False,
        "fails_sum": 0,
        "has_fails": False,
        "min_val": None,   # track global min
        "has_min": False,
        "max_val": None,   # track global max
        "has_max": False,

        # We'll track sums of naive "avg", "med", "p(90)", "p(95)", "value"
        # plus how many docs contributed to each
        "avg_sum": 0.0,
        "avg_count": 0,
        "med_sum": 0.0,
        "med_count": 0,
        "p90_sum": 0.0,
        "p90_count": 0,
        "p95_sum": 0.0,
        "p95_count": 0,
        "value_sum": 0.0,
        "value_count": 0,

        # Tracking gauges which need to be summed and then min/maxed
        "gauge_sum_min": 0,
        "gauge_sum_max": 0,
        "gauge_sum_value": 0,

        # We'll compute 'rate' later after we know total_time
        # but we store a flag if it’s a "counter" that might have a rate
        "has_rate": (m_type == "counter"),
    }

def update_metric_aggregator(agg, values_dict):
    """
    Merge one document's .metric[<name>].values into the aggregator.
    We do not finalize anything here; just accumulate partial sums or global min/max.
    """

    # Handle "gauge" type metrics first and then fall through to the rest
    if agg["type"] == "gauge":
        agg["gauge_sum_min"] += values_dict.get("min", 0)
        agg["gauge_sum_max"] += values_dict.get("max", 0)
        agg["gauge_sum_value"] += values_dict.get("value", 0)
        return
        
    for k, v in values_dict.items():
        if k == "count":
            agg["count_sum"] += v
            agg["has_count"] = True
        elif k == "passes":
            agg["passes_sum"] += v
            agg["has_passes"] = True
        elif k == "fails":
            agg["fails_sum"] += v
            agg["has_fails"] = True
        elif k == "min":
            if agg["min_val"] is None or v < agg["min_val"]:
                agg["min_val"] = v
            agg["has_min"] = True
        elif k == "max":
            if agg["max_val"] is None or v > agg["max_val"]:
                agg["max_val"] = v
            agg["has_max"] = True
        elif k == "avg":
            agg["avg_sum"] += v
            agg["avg_count"] += 1
        elif k == "med":
            agg["med_sum"] += v
            agg["med_count"] += 1
        elif k == "p(90)":
            agg["p90_sum"] += v
            agg["p90_count"] += 1
        elif k == "p(95)":
            agg["p95_sum"] += v
            agg["p95_count"] += 1
        elif k == "value":
            agg["value_sum"] += v
            agg["value_count"] += 1
        elif k == "rate":
            # We'll compute rate at finalize time. We do not add or track partial rates here.
            pass
        # Default is to just drop values that don't exist in the aggregator.

def finalize_metric_aggregator(agg, total_time_s):
    """
    Produce the final .metrics[<name>] entry from the aggregator. We do the naive average for avg,
    med, p(90), p(95), value (sum / count). We do sum for count/passes/fails, global min/max, etc.
    We compute rate = count_sum / total_time_s if it's a counter.
    """
    final_values = {}

    # Handle gauges here. With gauges, everything else is set to default so nothing after this will be True
    if agg["type"] == "gauge":
        final_values["min"] = agg["gauge_sum_min"]
        final_values["max"] = agg["gauge_sum_max"]
        final_values["value"] = agg["gauge_sum_value"]

    # Summations
    if agg["has_count"]:
        final_values["count"] = agg["count_sum"]
    if agg["has_passes"]:
        final_values["passes"] = agg["passes_sum"]
    if agg["has_fails"]:
        final_values["fails"] = agg["fails_sum"]

    # Min/max
    if agg["has_min"]:
        final_values["min"] = agg["min_val"]
    if agg["has_max"]:
        final_values["max"] = agg["max_val"]

    # Naive average fields
    if agg["avg_count"] > 0:
        final_values["avg"] = agg["avg_sum"] / agg["avg_count"]
    if agg["med_count"] > 0:
        final_values["med"] = agg["med_sum"] / agg["med_count"]
    if agg["p90_count"] > 0:
        final_values["p(90)"] = agg["p90_sum"] / agg["p90_count"]
    if agg["p95_count"] > 0:
        final_values["p(95)"] = agg["p95_sum"] / agg["p95_count"]
    if agg["value_count"] > 0:
        final_values["value"] = agg["value_sum"] / agg["value_count"]

    # If this is a counter that might have a rate, compute it from count / total_time_s
    # (assuming distributed tests in parallel)
    if agg["has_rate"] and agg["has_count"] and total_time_s > 0:
        final_values["rate"] = final_values["count"] / total_time_s

    return {
        "type": agg["type"],
        "contains": agg["contains"],
        "values": final_values
    }

def combine_all_docs(docs):
    """
    1) Find maximum .state.testRunDurationMs (assuming parallel test).
    2) Aggregate metrics from all docs in a single pass.
    3) Finalize the aggregates into one set of .metrics.
    4) Return the final merged doc.
    """
    if not docs:
        return {}

    # 1) Determine the global "test duration" (max of testRunDurationMs).
    max_duration_ms = 0
    for doc in docs:
        state = doc.get("state", {})
        dur = state.get("testRunDurationMs", 0)
        if dur > max_duration_ms:
            max_duration_ms = dur
    total_time_s = max_duration_ms / 1000.0

    # 2) Build aggregators for each metric from all docs
    metric_aggregators = {}  # metric_name -> aggregator
    for doc in docs:
        metrics = doc.get("metrics", {})
        for m_name, m_data in metrics.items():
            m_type = m_data.get("type", "")
            m_contains = m_data.get("contains", "")

            if m_name not in metric_aggregators:
                metric_aggregators[m_name] = create_metric_aggregator(m_type, m_contains)

            # Update aggregator with this doc's values
            values_dict = m_data.get("values", {})
            update_metric_aggregator(metric_aggregators[m_name], values_dict)

    # 3) Finalize each aggregator -> produce final metrics
    merged_metrics = {}
    for m_name, agg in metric_aggregators.items():
        merged_metrics[m_name] = finalize_metric_aggregator(agg, total_time_s)

    # 4) Construct the final merged doc
    # We keep root_group/options from the first doc; override testRunDurationMs with max
    first = docs[0]
    merged = {
        "root_group": first.get("root_group", {}),
        "options": first.get("options", {}),
        "state": {
            **first.get("state", {}),
            "testRunDurationMs": max_duration_ms
        },
        "metrics": merged_metrics
    }
    return merged

def main():
    # Read a single JSON array from stdin
    raw = sys.stdin.read().strip()
    if not raw:
        print("No input data")
        sys.exit(1)

    docs = json.loads(raw)
    if not isinstance(docs, list):
        print("Input JSON is not an array")
        sys.exit(1)

    merged = combine_all_docs(docs)
    print(json.dumps(merged, indent=2))

if __name__ == "__main__":
    main()
