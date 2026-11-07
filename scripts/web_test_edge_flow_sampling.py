"""Pure observability checks for captured edge-flow pixels.

Sampling readiness has no expected direction. All captured frames, including
reverse motion and clipped heads, remain available to the independent
direction oracle in web_test_edge_flow.
"""

import math


def _inside(marker, region):
    bounds = marker["bounds"]
    return (region[0] < bounds[0] and region[1] < bounds[1]
            and bounds[2] < region[2] and bounds[3] < region[3])


def sampling_readiness(frames, moving_regions, max_interval):
    timing_pairs = [index for index, (before, after) in enumerate(zip(frames, frames[1:]))
                    if 0 < after["capture_timestamp_seconds"] - before["capture_timestamp_seconds"] < max_interval]
    motion_pairs = {}
    for region in moving_regions:
        observable = []
        for index in timing_pairs:
            before, after = frames[index]["regions"][region], frames[index + 1]["regions"][region]
            for left in before["markers"]:
                # Match before filtering: a stationary or clipped nearest head
                # must not be replaced by another head to manufacture motion.
                right = min(after["markers"], default=None,
                            key=lambda marker: math.dist(left["center"], marker["center"]))
                if (right is not None and _inside(left, before["bounds"])
                        and _inside(right, after["bounds"])
                        and 1.5 < math.dist(left["center"], right["center"]) < 25):
                    observable.append(index)
                    break
        motion_pairs[region] = observable
    return {"frame_count": len(frames), "timing_pairs": timing_pairs,
            "motion_pairs": motion_pairs, "max_interval": max_interval,
            "ready": bool(moving_regions) and len(frames) >= 10 and len(timing_pairs) >= 6
                     and all(len(pairs) >= 2 for pairs in motion_pairs.values())}


def capture_interval(max_interval):
    # The production heads repeat every 56 / 60 seconds. Pace actual CDP
    # acknowledgements so a fast compositor cannot exhaust 24 captures before
    # an initially clipped head has another full opportunity to cross the gap.
    # The anti-aliasing bound remains stricter when the visible region is small.
    return min(56 / (60 * 12), max_interval / 3)
