//! libstdc++ `std::sort` replica (introsort, threshold 16).
//!
//! GEOS orders STRtree nodes with `std::sort`, which is not stable: items
//! with equal keys come out in an order determined by the algorithm. The
//! DRC port reproduces shapely's query order, so it needs the identical
//! permutation, not just a sorted one.

const THRESHOLD: usize = 16;

fn lg(n: usize) -> usize {
    (usize::BITS - 1 - n.leading_zeros()) as usize
}

/// Sort `v` exactly as libstdc++ `std::sort(v.begin(), v.end(), less)`.
pub fn std_sort<T, F: Fn(&T, &T) -> bool>(v: &mut [T], less: &F) {
    let n = v.len();
    if n > 1 {
        introsort_loop(v, 0, n, 2 * lg(n), less);
        final_insertion_sort(v, 0, n, less);
    }
}

fn introsort_loop<T, F: Fn(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    mut last: usize,
    mut depth: usize,
    less: &F,
) {
    while last - first > THRESHOLD {
        if depth == 0 {
            partial_sort(v, first, last, less);
            return;
        }
        depth -= 1;
        let cut = unguarded_partition_pivot(v, first, last, less);
        introsort_loop(v, cut, last, depth, less);
        last = cut;
    }
}

fn move_median_to_first<T, F: Fn(&T, &T) -> bool>(
    v: &mut [T],
    result: usize,
    a: usize,
    b: usize,
    c: usize,
    less: &F,
) {
    if less(&v[a], &v[b]) {
        if less(&v[b], &v[c]) {
            v.swap(result, b);
        } else if less(&v[a], &v[c]) {
            v.swap(result, c);
        } else {
            v.swap(result, a);
        }
    } else if less(&v[a], &v[c]) {
        v.swap(result, a);
    } else if less(&v[b], &v[c]) {
        v.swap(result, c);
    } else {
        v.swap(result, b);
    }
}

fn unguarded_partition<T, F: Fn(&T, &T) -> bool>(
    v: &mut [T],
    mut first: usize,
    mut last: usize,
    pivot: usize,
    less: &F,
) -> usize {
    loop {
        while less(&v[first], &v[pivot]) {
            first += 1;
        }
        last -= 1;
        while less(&v[pivot], &v[last]) {
            last -= 1;
        }
        if first >= last {
            return first;
        }
        v.swap(first, last);
        first += 1;
    }
}

fn unguarded_partition_pivot<T, F: Fn(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    last: usize,
    less: &F,
) -> usize {
    let mid = first + (last - first) / 2;
    move_median_to_first(v, first, first + 1, mid, last - 1, less);
    unguarded_partition(v, first + 1, last, first, less)
}

fn unguarded_linear_insert<T, F: Fn(&T, &T) -> bool>(v: &mut [T], mut last: usize, less: &F) {
    // Rotate the element at `last` leftwards while it is less than its
    // predecessor (equivalent to the move-based libstdc++ loop).
    while last > 0 && less(&v[last], &v[last - 1]) {
        v.swap(last, last - 1);
        last -= 1;
    }
}

fn insertion_sort<T, F: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, less: &F) {
    if first == last {
        return;
    }
    for i in first + 1..last {
        if less(&v[i], &v[first]) {
            v[first..=i].rotate_right(1);
        } else {
            unguarded_linear_insert(v, i, less);
        }
    }
}

fn final_insertion_sort<T, F: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, less: &F) {
    if last - first > THRESHOLD {
        insertion_sort(v, first, first + THRESHOLD, less);
        for i in first + THRESHOLD..last {
            unguarded_linear_insert(v, i, less);
        }
    } else {
        insertion_sort(v, first, last, less);
    }
}

// --- heap (std::__partial_sort with middle == last) ---

fn push_heap<T, F: Fn(&T, &T) -> bool>(v: &mut [T], base: usize, mut hole: usize, top: usize, less: &F) {
    // `v[base + hole]` holds the value being pushed.
    let mut parent = if hole > 0 { (hole - 1) / 2 } else { 0 };
    while hole > top && less(&v[base + parent], &v[base + hole]) {
        v.swap(base + hole, base + parent);
        hole = parent;
        if hole == 0 {
            break;
        }
        parent = (hole - 1) / 2;
    }
}

fn adjust_heap<T, F: Fn(&T, &T) -> bool>(v: &mut [T], base: usize, mut hole: usize, len: usize, less: &F) {
    let top = hole;
    let mut child = hole;
    while child < (len - 1) / 2 {
        child = 2 * (child + 1);
        if less(&v[base + child], &v[base + child - 1]) {
            child -= 1;
        }
        v.swap(base + hole, base + child);
        hole = child;
    }
    if len % 2 == 0 && child == (len - 2) / 2 {
        child = 2 * (child + 1);
        v.swap(base + hole, base + child - 1);
        hole = child - 1;
    }
    push_heap(v, base, hole, top, less);
}

fn partial_sort<T, F: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, less: &F) {
    let len = last - first;
    if len < 2 {
        return;
    }
    // make_heap
    let mut parent = (len - 2) / 2;
    loop {
        adjust_heap(v, first, parent, len, less);
        if parent == 0 {
            break;
        }
        parent -= 1;
    }
    // sort_heap
    let mut end = len;
    while end > 1 {
        end -= 1;
        v.swap(first, first + end);
        adjust_heap(v, first, 0, end, less);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts() {
        let mut v: Vec<i64> = (0..500).map(|i| (i * 7919) % 101).collect();
        std_sort(&mut v, &|a, b| a < b);
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
    }
}
