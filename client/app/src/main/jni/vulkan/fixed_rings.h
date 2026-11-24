#pragma once

#include <cstddef>
#include <cstdint>

namespace vkr {

// Fixed-capacity ordered ring buffer keyed by monotonically increasing (or mostly ordered) keys.
// Replaces std::map on hot per-frame paths (pendingPresents_, origins_) with zero heap allocations
// and O(1) append/FIFO-pop/prefix-discard.
template <typename Key, typename Value, size_t Capacity>
class FixedOrderedRing {
    static_assert(Capacity >= 4 && (Capacity & (Capacity - 1)) == 0,
                  "FixedOrderedRing capacity must be a power of two >= 4");
    static constexpr size_t kMask = Capacity - 1;

public:
    struct Entry {
        Key key {};
        Value value {};
    };

    bool empty() const { return count_ == 0; }
    size_t size() const { return count_; }
    void clear() { head_ = count_ = 0; }

    Entry& front() { return entries_[head_]; }
    const Entry& front() const { return entries_[head_]; }

    void popFront() {
        if (count_ == 0) return;
        head_ = (head_ + 1) & kMask;
        count_--;
    }

    Value& put(Key key, const Value& value) {
        if (count_ == 0 || key > at(count_ - 1).key) {
            if (count_ == Capacity) popFront();
            Entry& dst = at(count_);
            dst.key = key;
            dst.value = value;
            count_++;
            return dst.value;
        }
        size_t pos = lowerBound(key);
        if (pos < count_ && at(pos).key == key) {
            at(pos).value = value;
            return at(pos).value;
        }
        if (count_ == Capacity) {
            popFront();
            if (pos > 0) pos--;
        }
        for (size_t i = count_; i > pos; --i) {
            at(i) = at(i - 1);
        }
        Entry& dst = at(pos);
        dst.key = key;
        dst.value = value;
        count_++;
        return dst.value;
    }

    Value* find(Key key) {
        const size_t pos = lowerBound(key);
        if (pos < count_ && at(pos).key == key) {
            return &at(pos).value;
        }
        return nullptr;
    }

    bool erase(Key key) {
        if (count_ == 0) return false;
        if (at(0).key == key) {
            popFront();
            return true;
        }
        const size_t pos = lowerBound(key);
        if (pos >= count_ || at(pos).key != key) return false;
        for (size_t i = pos; i + 1 < count_; ++i) {
            at(i) = at(i + 1);
        }
        count_--;
        return true;
    }

    // Finds key, copies its value to *out, and drops every entry up to and including key in O(1).
    bool popThrough(Key key, Value* out) {
        const size_t pos = lowerBound(key);
        if (pos >= count_ || at(pos).key != key) return false;
        if (out) *out = at(pos).value;
        head_ = (head_ + pos + 1) & kMask;
        count_ -= (pos + 1);
        return true;
    }

private:
    Entry& at(size_t offset) { return entries_[(head_ + offset) & kMask]; }
    const Entry& at(size_t offset) const { return entries_[(head_ + offset) & kMask]; }

    size_t lowerBound(Key key) const {
        size_t lo = 0;
        size_t hi = count_;
        while (lo < hi) {
            const size_t mid = lo + ((hi - lo) >> 1);
            if (at(mid).key < key) lo = mid + 1;
            else hi = mid;
        }
        return lo;
    }

    Entry entries_[Capacity] {};
    size_t head_ = 0;
    size_t count_ = 0;
};

}  // namespace vkr
