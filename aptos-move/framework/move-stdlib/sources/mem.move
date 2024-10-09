/// Module with methods for safe memory manipulation.
module std::mem {
    /// Swap contents of two passed mutable references.
    ///
    /// Move prevents from having two mutable references to the same value,
    /// so `left` and `right` references are always distinct.
    public native fun swap<T>(left: &mut T, right: &mut T);

    /// Replace the value reference points to with the given new value,
    /// and return the value it had before.
    public fun replace<T>(ref: &mut T, new: T): T {
        swap(ref, &mut new);
        new
    }

   spec swap<T>(left: &mut T, right: &mut T) {
        pragma opaque;
        aborts_if false;
        ensures right == old(left);
        ensures left == old(right);
    }

    spec replace<T>(ref: &mut T, new: T): T {
        pragma opaque;
        aborts_if false;
        ensures result == old(ref);
        ensures ref == new;
    }
}
