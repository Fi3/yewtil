//! A Reference Counted Pointer optimized for use with Yew.

use std::ptr::NonNull;
use std::cell::Cell;
use std::fmt;
use std::ops::Deref;
use std::hash::{Hash, Hasher};
use failure::_core::cmp::Ordering;

type IsZero = bool;


#[derive(PartialEq, Debug)]
struct Node<T> {
    prev: Option<NonNull<Node<T>>>,
    element: T,
    count: Cell<usize>,
    next: Option<NonNull<Node<T>>>,
}

impl <T> Node<T> {
    /// Creates a new node
    fn new(element: T) -> Self {
        Node {
            prev: None,
            element,
            count: Cell::new(1),
            next: None,
        }
    }

    /// Puts the node behind a non-null pointer.
    fn into_not_null(self) -> NonNull<Self> {
        unsafe {
            NonNull::new_unchecked(Box::into_raw(
                Box::new(self)
            ))
        }
    }

    /// Gets the reference count of the node
    fn get_count(&self) -> usize {
        self.count.get()
    }

    /// Increments the reference count of the node.
    fn inc_count(&self) {
        let mut count = self.count.get();
        count += 1;
        self.count.set(count);
    }

    /// Decrements the reference count of the node.
    /// It will return true if the count hits zero.
    /// This can be used to determine if the node should be deallocated.
    fn dec_count(&self) -> IsZero {
        let mut count = self.count.get();
        count -= 1;
        self.count.set(count);
        count == 0
    }
}

/// Linked Reference Counted pointer
///
/// A doubly linked list where the head represents the value presented to the world via
/// as_ref, or get_mut(), or make_mut().
/// The remaining elements represent shared pointers whose values have changed.
/// A Lrc pointer can swap its value to the newest modification along the chain.
///
/// The LRC allows cheap cloning like an `Rc` pointer,
/// but mutating the value when it is shared will cause a new element to be allocated in its place.
/// This new node can access the previous ones, as long as the other Lrcs still exist.
///
/// So from a given Lrc, you can see how many copies of the value you have,
/// as well as how many modified copies exist, and what their values are by iterating over Lrc,
/// as it implements the `Iterator` trait.
pub struct Lrc<T> {
    head: Option<NonNull<Node<T>>>
}

impl <T> Lrc<T> {

    /// Allocates the element on the heap next to a reference counter and next and previous pointers.
    pub fn new(element: T) -> Self {
        let node = Node::new(element);
        Lrc {
            head: Some(node.into_not_null())
        }
    }


    /// Sets a new value as the head, pushing the previous head to the second node in the list.
    ///
    /// This will not allocate if this Lrc has exclusive access to the node whose value is being set.
    /// It will update the head nodes value in that case.
    ///
    /// If the Lrc's head is shared with another Lrc, it will push a new node onto its head containing
    /// the new value. Unless the Lrc is cloned, or another Lrc updates to have this value, it will have
    /// exclusive access over this node, and calling set will remain cheap.
    ///
    /// # Example
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let mut lrc = Lrc::new(0);
    /// lrc.set(1);
    /// assert_eq!(lrc.as_ref(), &1);
    /// ```
    pub fn set(&mut self, element: T) {
        if self.is_exclusive() {
            // Directly assign the element if the ptr has exclusive access.
            self.get_mut_head_node().element = element;
        } else {
            // If the ptr is shared, allocate a new node.
            self.push_head(Node::new(element));
        }
    }

    /// Set the head with a new item using a reference to the current head.
    ///
    /// # Example
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let mut lrc = Lrc::new(0);
    /// lrc.alter(|current| current + 1);
    /// assert_eq!(lrc.as_ref(), &1);
    /// ```
    pub fn alter<F: Fn(&T) -> T>(&mut self, f: F) {
        let current_head_value = &self.get_ref_head_node().element;
        let new_head_value = f(current_head_value);
        self.set(new_head_value)
    }


    /// Gets a mutable reference to the owned value if this Lrc has exclusive ownership over its data.
    ///
    /// # Example
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let mut lrc = Lrc::new(1);
    ///
    /// let inner = lrc.get_mut();
    /// assert_eq!(inner, Some(&mut 1));
    ///
    /// let lrc_clone = lrc.clone();
    ///
    /// let inner = lrc.get_mut();
    /// assert_eq!(inner, None, "Can't get reference because lrc doesn't have exclusive ownership.");
    /// ```
    pub fn get_mut(&mut self) -> Option<&mut T> {
        if self.is_exclusive() {
            let node = self.get_mut_head_node();
            // Only this node has ownership, or it is marked dead.
            Some(&mut node.element)
        } else {
            None
        }
    }

    #[allow(dead_code)]
    // TODO finish implementing this.
    fn try_unwrap(self) -> Result<T, Self> {
        if self.is_exclusive() {
            let head: NonNull<Node<T>> = self.head.unwrap();
            unsafe {
                // TODO the expectation would be to use Option<T> in the node so it can be taken here.
//                let mut element: T = std::mem::zeroed().unwrap(); // TODO dropping a zeroed representation is certianly going to cause undefined behavior for any T that uses pointers.
//                std::mem::swap(&(*head.as_mut()).element, &mut element);
                let element = unimplemented!();

                (*head.as_ptr()).prev.as_mut().map(|prev| {
                    prev.as_mut().next = (*head.as_ptr()).next.take();
                });

                (*head.as_ptr()).next.as_mut().map(|next| {
                    next.as_mut().prev = (*head.as_ptr()).prev.take();
                });

                std::ptr::drop_in_place(head.as_ptr());

                Ok(element)
            }
        } else {
            Err(self)
        }
    }

    /// If this Lrc is shared, and one or more of its shared Lrcs has been modified,
    /// this will update this lrc to have the most up-to-date value (held currently by one of its clones).
    ///
    /// # Example
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let mut lrc = Lrc::new(0);
    ///
    /// let mut cloned_lrc = lrc.clone();
    /// cloned_lrc.set(1);
    /// assert_eq!(lrc.as_ref(), &0);
    ///
    /// lrc.update();
    /// assert_eq!(lrc.as_ref(), &1);
    /// ```
    pub fn update(&mut self) {
        while let Some(prev) = self.next_back() {
            *self = prev;
        }
    }

    /// Gets a prior value the pointer had (if any).
    ///
    /// It walks down the list, replacing the head as it creates a new Lrc.
    fn older(&self) -> Option<Self> {
        self.get_ref_head_node().next.map(|ptr| {
            unsafe {ptr.as_ref().inc_count();}
            Lrc {
                head: Some(ptr)
            }
        })
    }

    /// Gets a newer value the pointer has had (if any).
    ///
    /// It walks up the list, replacing the head as it creates a new Lrc.
    fn newer(&self) -> Option<Self> {
        self.get_ref_head_node().prev.map(|ptr| {
            unsafe {ptr.as_ref().inc_count();}
            Lrc {
                head: Some(ptr)
            }
        })
    }

    /// Compares head pointers for equality.
    ///
    /// # Example
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let lrc1 = Lrc::new(10);
    /// let lrc2 = Lrc::new(10);
    ///
    /// assert!(lrc1 == lrc2, "Values are the same");
    /// assert!(!Lrc::ptr_eq(&lrc1, &lrc2), "But they are behind different pointers");
    /// ```
    pub fn ptr_eq(lhs: &Self, rhs: &Self) -> bool {
        lhs.head.unwrap().eq(&rhs.head.unwrap())
    }

    /// Push a new node to the head of the Lrc.
    /// Because the head represents the active value for the Lrc,
    /// it effectively marks the old head for deletion if it wasn't already copied.
    fn push_head(&mut self, mut node: Node<T>) {
        debug_assert_eq!(node.prev, None);
        node.next = self.head;
        let node = Some(node.into_not_null());

        unsafe {
            match self.head {
                None => {}
                Some(head) => {
                    (*head.as_ptr()).prev = node;
                    // Decrement the count when a node is moved away from the head position.
                    // Unless it is owned by a cloned lrc, this will mark it as dead, and it will be pruned
                    // the next time `prune_dead_nodes` is run.
                    if (*head.as_ptr()).dec_count() {
                        // TODO remove this dead code if no one ever manages to trigger it.
                        debug_assert!(false, "This code should be dead, due to a condition in set that prevents push_head from being called when the count == 1");
                        // This is what should happen anyways, but reaching this instruction should be impossible.
                        std::ptr::drop_in_place(head.as_ptr());
                    }
                },
            }
        }

        self.head = node;
    }

    /// Gets the reference count of the head node.
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let lrc = Lrc::new(1);
    /// let count = (&lrc).get_count();
    /// assert_eq!(count, 1);
    ///
    /// let _lrc_clone_1 = lrc.clone();
    /// let count = (&lrc).get_count();
    /// assert_eq!(count, 2);
    ///
    /// let _lrc_clone_2 = lrc.clone();
    /// let count = (&lrc).get_count();
    /// assert_eq!(count, 3);
    /// ```
    pub fn get_count(&self) -> usize {
        self.get_ref_head_node().get_count()
    }

    /// Returns true if no other Lrcs point to the head node.
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let lrc = Lrc::new(1);
    /// assert!(lrc.is_exclusive());
    /// let _lrc_clone = lrc.clone();
    /// assert!(!lrc.is_exclusive());
    /// ```
    pub fn is_exclusive(&self) -> bool {
        self.get_count() == 1
    }

    // O(n) operation to determine how long the list is.
    pub fn len(&self) -> usize {
        let mut count = 1;

        unsafe {
            let mut node = self.get_ref_head_node();

            while let Some(next_node) = node.next.as_ref() {
                count += 1;
                node = next_node.as_ref()
            }
        }
        count
    }

    /// Gets a mutable reference to the head node.
    fn get_mut_head_node(&mut self) -> &mut Node<T> {
        unsafe {
            self.head.as_mut().unwrap().as_mut()
        }
    }

    /// Gets a reference to the head node.
    fn get_ref_head_node(&self) -> &Node<T> {
        unsafe {
            self.head.as_ref().unwrap().as_ref()
        }
    }

}

impl <T: Clone> Lrc<T> {
    /// Provides a mutable reference to the head's value.
    /// If the head is shared with another LRC, this will clone the head to ensure exclusive access.
    ///
    /// # Example
    /// ```
    ///# use yewtil::lrc::Lrc;
    /// let mut lrc = Lrc::new(1);
    /// let _lrc_clone = lrc.clone();
    ///
    /// assert_eq!((&lrc).get_count(), 2, "There are two Lrcs pointing to the same data.");
    /// assert_eq!(lrc.len(), 1, "The Lrc has a single node.");
    ///
    /// *lrc.make_mut() = 2;
    /// assert_eq!((&lrc).get_count(), 1, "This Lrc has exclusive ownership of this data.");
    /// assert_eq!(lrc.len(), 2, "The other lrc is pointing to the node that holds the value '1'.");
    ///
    /// *lrc.make_mut() = 3;
    /// assert_eq!(lrc.len(), 2, "This Lrc is still exclusive, so no more allocations are needed.");
    /// ```
    pub fn make_mut(&mut self) -> &mut T {
        // Use this to smuggle the copy past the borrow checker.
        if self.get_count() > 1 {
            // Clone to ensure unique ownership
            let cloned_element = self.get_ref_head_node().element.clone();
            self.push_head(Node::new(cloned_element))
        }
        &mut self.get_mut_head_node().element
    }
}

impl <T> Drop for Lrc<T> {
    fn drop(&mut self) {
        let head = self.head.expect("Head should always be present.");
        unsafe {
            // If the heads ref-count is 0
            if head.as_ref().dec_count() {
                // Attach surrounding nodes to each other as this one is removed.
                (*head.as_ptr()).prev.as_mut().map(|prev| {
                    prev.as_mut().next = (*head.as_ptr()).next.take();
                });

                (*head.as_ptr()).next.as_mut().map(|next| {
                    next.as_mut().prev = (*head.as_ptr()).prev.take();
                });

                std::ptr::drop_in_place(head.as_ptr());
            }
        }
    }
}
impl <T> Clone for Lrc<T> {
    fn clone(&self) -> Self {
        if let Some(head) = self.head {
            unsafe {
                head.as_ref().inc_count();
            }
        }
        Lrc {
            head: self.head.clone()
        }
    }
}

impl <T: PartialEq> PartialEq for Lrc<T> {
    fn eq(&self, other: &Self) -> bool {
        // TODO refactor this to remove the unsafe block.
        unsafe{
            match (self.head, other.head) {
                (Some(lhs), Some(rhs)) => {
                    lhs.as_ref().element.eq(&rhs.as_ref().element)
                }
                _ => false
            }
        }
    }
}

impl <T: Eq> Eq for Lrc<T> {}

impl <T: PartialOrd> PartialOrd for Lrc<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.get_ref_head_node().element.partial_cmp(&other.get_ref_head_node().element)
    }
}
impl <T: Ord> Ord for Lrc<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.get_ref_head_node().element.cmp(&other.get_ref_head_node().element)
    }
}

impl <T: Hash> Hash for Lrc<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.get_ref_head_node().element.hash(state)
    }
}

impl <T> AsRef<T> for Lrc<T> {
    fn as_ref(&self) -> &T {
        &self.get_ref_head_node().element
    }
}

impl <T> Deref for Lrc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.get_ref_head_node().element
    }
}

impl <T: fmt::Debug> fmt::Debug for Lrc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Lrc").field(&self.head).finish()
    }
}

impl <T> Iterator for Lrc<T> {
    type Item = Lrc<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.older()
    }
}

impl <T> DoubleEndedIterator for Lrc<T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.newer()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn lrc_new() {
        let lrc = Lrc::new(25);
        assert_eq!(lrc, Lrc{head: Some(Node::new(25).into_not_null())});
        assert_eq!(lrc.as_ref(), &25)
    }

    #[test]
    fn clone_lrc() {
        let lrc = Lrc::new(25);
        let copy = lrc.clone();
        assert_eq!(copy.as_ref(), &25)
    }

    #[test]
    fn set_lrc() {
        let mut lrc = Lrc::new(25);
        lrc.set(30);

        assert_eq!(lrc.as_ref(), &30)
    }

    #[test]
    fn len_1() {
        let mut lrc = Lrc::new(25);
        lrc.set(24);
        assert_eq!(lrc.len(), 1);
    }


    #[test]
    fn droping_middle_connects_prev_and_next() {
        let mut lrc = Lrc::new(0);
        assert_eq!(lrc.get_ref_head_node().count, Cell::new(1), "exclusive ownership");

        // Clone the initial value so it will stick around towards the end of this test
        let _og_clone = lrc.clone();
        assert_eq!(lrc.get_ref_head_node().count, Cell::new(2), "shared ownership");

        lrc.set(1);

        assert_eq!(lrc.get_ref_head_node().prev, None);
        assert_eq!(lrc.get_ref_head_node().element, 1);
        assert_eq!(lrc.get_ref_head_node().count, Cell::new(1));
        assert!(lrc.get_ref_head_node().next.is_some(), "Should have pointer to previous head");

        unsafe {
            let lrcs_next = lrc.get_ref_head_node().next.as_ref().expect("Should have next node").as_ref();
            let lrcs_nexts_prev = lrcs_next.prev.as_ref().expect("next.prev should be some").as_ref();

            assert_eq!(lrcs_next.element, 0);
            assert_eq!(lrcs_next.count, Cell::new(1), "Should still be owned by the Og Clone");
            assert!(lrcs_next.prev.is_some(), "Should point to head");

            assert_eq!(lrcs_nexts_prev, lrc.get_ref_head_node(), "the head's next ptr's prev ptr should point back to the head");
        }

        // Clone the head.
        let cloned_lrc = lrc.clone();
        assert_eq!(lrc.len(), 2);

        assert_eq!(cloned_lrc.get_ref_head_node().prev, None);
        assert_eq!(cloned_lrc.get_ref_head_node().element, 1);
        assert_eq!(cloned_lrc.get_ref_head_node().count, Cell::new(2));
        assert!(cloned_lrc.get_ref_head_node().next.is_some(), "Should have pointer to previous head");

        // Replace the head again
        lrc.set(2);

        assert_eq!(lrc.get_ref_head_node().prev, None);
        assert_eq!(lrc.get_ref_head_node().element, 2, "value should now be updated to 2");
        assert_eq!(lrc.get_ref_head_node().count, Cell::new(1), "there should only be one owner of this node");
        assert!(lrc.get_ref_head_node().next.is_some(), "Should have pointer to previous head");

        unsafe {
            // This should have modified the cloned_lrc's head's prev ptr to point to the head of the lrc
            let cloned_lrcs_heads_prev_value = cloned_lrc.get_ref_head_node().prev.as_ref().expect("Should point to head").as_ref();
            assert_eq!(cloned_lrcs_heads_prev_value, lrc.get_ref_head_node());
        }

        assert_eq!(lrc.len(), 3);

        // Drop the cloned_lrc, which in cleanup,
        // should attach the head node of lrc (currently of value 2),
        // with the lail node of lrc (value of 0)
        std::mem::drop(cloned_lrc);

        assert_eq!(lrc.len(), 2);

        unsafe {
            let lrcs_next = lrc.get_ref_head_node().next.as_ref().expect("Should have next node").as_ref();
            assert_eq!(lrcs_next.element, 0);
        }
    }


//    #[test]
//    fn set_prune() {
//        let mut lrc = Lrc::new(25);
//        lrc.set_and_prune(24);
//        assert_eq!(lrc.len(), 1);
//    }

    #[test]
    fn single_node_older_yeilds_none() {
        let lrc = Lrc::new(25);
        let older = lrc.older();
        assert_eq!(older, None)
    }

    #[test]
    fn single_node_newer_yeilds_none() {
        let lrc = Lrc::new(25);
        let newer = lrc.newer();
        assert_eq!(newer, None)
    }

    #[test]
    fn older_traverses_to_previous_lrc() {
        let mut lrc = Lrc::new(25);
        let _clone = lrc.clone();
        lrc.set(26);
        let older = lrc.older();
        assert_eq!(older, Some(Lrc::new(25)))
    }

    #[test]
    fn newer_traverses_back_to_original_head_lrc() {
        let mut lrc = Lrc::new(25);
        let _clone = lrc.clone();
        lrc.set(26);
        let older = lrc.older();
        assert_eq!(older, Some(Lrc::new(25)));
        let newer = older.unwrap().newer();
        assert_eq!(newer, Some(lrc));
    }

    #[test]
    fn attempt_to_dangle_ref() {
        let lrc = Lrc::new(vec![25]);
        let mut cloned_lrc = lrc.clone();
        let first_item_ref = &lrc.as_ref()[0];
        cloned_lrc.set(vec![22, 23]);
        assert_eq!(first_item_ref, &25)
    }

    #[test]
    fn ptr_eq_positive() {
        let lrc = Lrc::new(24);
        let cloned_lrc = lrc.clone();

        assert!(Lrc::ptr_eq(&lrc, &cloned_lrc));
    }

    #[test]
    fn ptr_eq_negative() {
        let lrc = Lrc::new(24);
        let other_lrc = Lrc::new(24);

        assert!(!Lrc::ptr_eq(&lrc, &other_lrc));
    }

    #[test]
    fn update_sets_lrc_to_have_newest_value() {
        let mut lrc = Lrc::new(0);
        let mut cloned_lrc = lrc.clone();

        cloned_lrc.set(1);
        assert_eq!(cloned_lrc.as_ref(), &1);
        assert_eq!(lrc.as_ref(), &0);

        lrc.update();
        assert_eq!(lrc.as_ref(), &1);
    }
}