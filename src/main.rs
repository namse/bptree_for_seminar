#[repr(C, align(64))]
struct Page {
    bytes: [u8; 4096],
}
impl Page {
    fn is_leaf_node(&self) -> bool {
        self.bytes[0] == 1
    }

    fn as_internal_node(&self) -> &InternalNode {
        unsafe { &*(self.bytes.as_ptr() as *const InternalNode) }
    }

    fn as_leaf_node_mut(&mut self) -> &mut LeafNode {
        unsafe { &mut *(self.bytes.as_mut_ptr() as *mut LeafNode) }
    }

    fn as_internal_node_mut(&mut self) -> &mut InternalNode {
        unsafe { &mut *(self.bytes.as_mut_ptr() as *mut InternalNode) }
    }

    fn as_leaf_node(&self) -> &LeafNode {
        unsafe { &*(self.bytes.as_ptr() as *const LeafNode) }
    }

    fn as_header_mut(&mut self) -> &mut Header {
        unsafe { &mut *(self.bytes.as_mut_ptr() as *mut Header) }
    }

    fn as_header(&self) -> &Header {
        unsafe { &*(self.bytes.as_ptr() as *const Header) }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
struct PageOffset {
    value: u32,
}
impl PageOffset {
    fn new(value: u32) -> Self {
        Self { value }
    }
}

impl PageOffset {
    const HEADER: PageOffset = PageOffset { value: 0 };
    const NULL: PageOffset = PageOffset { value: u32::MAX };
}

#[repr(C, align(64))]
struct Header {
    root_offset: PageOffset,
    _padding: [u8; 4096 - 4],
}
impl Header {
    fn new(root_offset: PageOffset) -> Self {
        Self {
            root_offset,
            _padding: [0; 4096 - 4],
        }
    }
    fn into_page(self) -> Page {
        Page {
            bytes: unsafe { std::mem::transmute::<Self, [u8; 4096]>(self) },
        }
    }
}

struct IdSet {
    pages: Vec<Page>,
    // 0번 페이지를 root로 써선 안됩니다! 왜냐하면 root가 split되면 새로운 root가 생기니까요.
    // 그래서 움직일 일이 없는 header를 0번 페이지에 두고, header에 root의 offset을 저장하는 방식으로 하겠습니다.
}
impl IdSet {
    fn new() -> Self {
        Self {
            pages: vec![
                Header::new(PageOffset::new(1)).into_page(),
                LeafNode::new().into_page(),
            ],
        }
    }

    fn insert(&mut self, id: u128) {
        let (leaf_node_offset, mut parent_offsets) = self.find_leaf_node_to_insert(id);

        let leaf_node = self.page_mut(leaf_node_offset).as_leaf_node_mut();

        if !leaf_node.is_full() {
            leaf_node.insert(id);
            return;
        }

        let (new_right_node, center_id) = leaf_node.insert_and_split(id);
        let right_node_offset = self.allocate_new_page(new_right_node.into_page());

        // 여기서부터 본격적으로 internal node에 집어넣는 작업이 시작

        let mut left_node_offset = leaf_node_offset;
        let mut right_node_offset = right_node_offset;
        let mut center_id = center_id;

        loop {
            let Some(parent_offset) = parent_offsets.pop() else {
                // pop했는데 아무것도 없다면, 이미 root 노드까지 온거고, root가 split된거야.

                let new_root = InternalNode::new(center_id, left_node_offset, right_node_offset);
                let new_root_offset = self.allocate_new_page(new_root.into_page());
                self.header_mut().root_offset = new_root_offset;
                return;
            };

            let internal_node = self.page_mut(parent_offset).as_internal_node_mut();

            if !internal_node.is_full() {
                internal_node.insert(center_id, right_node_offset);
                return;
            }

            // parent_offset의 internal_node 가 쪼개지고
            let (right_node, new_center_id) =
                internal_node.insert_split(center_id, right_node_offset);

            right_node_offset = self.allocate_new_page(right_node.into_page());
            center_id = new_center_id;
            // 그러니 parent_offset 가 다음의 left_node_offset가 되는 것은 당연지사
            left_node_offset = parent_offset;
        }
    }

    fn contains(&self, id: u128) -> bool {
        let (leaf_node_offset, _) = self.find_leaf_node_to_insert(id);

        let leaf_node = self.page(leaf_node_offset).as_leaf_node();

        leaf_node.ids.iter().any(|&id_| id == id_)
    }

    fn find_leaf_node_to_insert(&self, id: u128) -> (PageOffset, Vec<PageOffset>) {
        let mut parent_offsets = vec![];

        let mut page_offset = self.header().root_offset;

        loop {
            let page = self.page(page_offset);
            if page.is_leaf_node() {
                return (page_offset, parent_offsets);
            }

            parent_offsets.push(page_offset);

            let internal_node = page.as_internal_node();
            page_offset = internal_node.find_offset_to_insert(id);
        }
    }

    fn page(&self, page_offset: PageOffset) -> &Page {
        &self.pages[page_offset.value as usize]
    }

    fn page_mut(&mut self, page_offset: PageOffset) -> &mut Page {
        &mut self.pages[page_offset.value as usize]
    }

    fn allocate_new_page(&mut self, page: Page) -> PageOffset {
        let page_offset = PageOffset {
            value: self.pages.len() as u32,
        };
        self.pages.push(page);
        page_offset
    }

    fn header_mut(&mut self) -> &mut Header {
        self.page_mut(PageOffset::HEADER).as_header_mut()
    }

    fn header(&self) -> &Header {
        self.page(PageOffset::HEADER).as_header()
    }
}

const INTERNAL_NODE_MAX_LEN: usize = 203;
#[repr(C, align(64))]
struct InternalNode {
    leaf_type: u8,
    _padding1: [u8; 7],
    id_count: u16,
    _padding: [u8; 6],
    ids: [u128; INTERNAL_NODE_MAX_LEN],
    child_offsets: [PageOffset; INTERNAL_NODE_MAX_LEN + 1],
}
impl InternalNode {
    /// Internal Node는 항상 M 자 모양이여야 하니까, 처음부터도 center id와 left, right 가 있어야겠죠?
    fn new(center_id: u128, left_node_offset: PageOffset, right_node_offset: PageOffset) -> Self {
        Self::new_from_ids(&[center_id], &[left_node_offset, right_node_offset])
    }
    fn new_from_ids(ids: &[u128], pages: &[PageOffset]) -> Self {
        assert_eq!(ids.len() + 1, pages.len());
        assert!(ids.len() <= INTERNAL_NODE_MAX_LEN);

        Self {
            leaf_type: 0,
            _padding1: [0; 7],
            id_count: ids.len() as u16,
            _padding: [0; 6],
            ids: {
                let mut ids_ = [0; INTERNAL_NODE_MAX_LEN];
                ids_[..ids.len()].copy_from_slice(ids);
                ids_
            },
            child_offsets: {
                let mut child_offsets = [PageOffset::NULL; INTERNAL_NODE_MAX_LEN + 1];
                child_offsets[..pages.len()].copy_from_slice(pages);
                child_offsets
            },
        }
    }
    fn find_offset_to_insert(&self, id: u128) -> PageOffset {
        let index = self
            .ids
            .iter()
            .position(|&id_| id < id_)
            .unwrap_or(self.id_count as usize);
        self.child_offsets[index]
    }

    fn into_page(self) -> Page {
        Page {
            bytes: unsafe { std::mem::transmute::<Self, [u8; 4096]>(self) },
        }
    }

    fn is_full(&self) -> bool {
        self.id_count == INTERNAL_NODE_MAX_LEN as u16
    }

    fn index_to_insert(&self, id: u128) -> usize {
        self.ids
            .iter()
            .position(|&id_| id < id_)
            .unwrap_or(self.id_count as usize)
    }

    fn insert(&mut self, id: u128, right_node_offset: PageOffset) {
        assert!(!self.is_full());

        let index = self.index_to_insert(id);

        // 오른쪽으로 한칸씩 밀고
        if index < self.id_count as usize {
            self.ids
                .copy_within(index..self.id_count as usize, index + 1);
            self.child_offsets
                .copy_within(index + 1..self.id_count as usize + 1, index + 2);
        }
        self.ids[index] = id; // 넣으면
        self.child_offsets[index + 1] = right_node_offset;

        // 정렬된 상태가 유지되겠죠~?

        self.id_count += 1;
    }

    fn insert_split(&mut self, id: u128, right_node_offset: PageOffset) -> (InternalNode, u128) {
        assert!(self.is_full());

        let index = self.index_to_insert(id);

        let one_more_ids = {
            let mut ids = [0; INTERNAL_NODE_MAX_LEN + 1];
            ids[..index].copy_from_slice(&self.ids[..index]);
            ids[index] = id;
            ids[index + 1..].copy_from_slice(&self.ids[index..]);
            ids
        };
        let one_more_offsets = {
            let mut offsets = [PageOffset::NULL; INTERNAL_NODE_MAX_LEN + 2];
            offsets[..index + 1].copy_from_slice(&self.child_offsets[..index + 1]);
            offsets[index + 1] = right_node_offset;
            offsets[index + 2..].copy_from_slice(&self.child_offsets[index + 1..]);
            offsets
        };

        let center_id_index = one_more_ids.len() / 2;
        let center_id = one_more_ids[center_id_index];

        let (left_ids, right_ids_include_center) = one_more_ids.split_at(center_id_index);
        let right_ids = &right_ids_include_center[1..];

        let (left_offsets, right_offsets) = one_more_offsets.split_at(center_id_index + 1);

        let right_node = InternalNode::new_from_ids(right_ids, right_offsets);

        *self = InternalNode::new_from_ids(left_ids, left_offsets);

        (right_node, center_id)
    }
}

const LEAF_NODE_MAX_LEN: usize = 255;
#[repr(C, align(64))]
struct LeafNode {
    leaf_type: u8,
    _padding1: [u8; 7],
    id_count: u16,
    _padding: [u8; 6],
    ids: [u128; LEAF_NODE_MAX_LEN],
}
impl LeafNode {
    fn new() -> Self {
        Self {
            leaf_type: 1,
            _padding1: [0; 7],
            id_count: 0,
            _padding: [0; 6],
            ids: [0; 255],
        }
    }
    fn new_from_ids(sorted_ids: &[u128]) -> Self {
        let mut leaf_node = Self::new();
        leaf_node.id_count = sorted_ids.len() as u16;
        leaf_node.ids[..sorted_ids.len()].copy_from_slice(sorted_ids);
        leaf_node
    }
    fn into_page(self) -> Page {
        Page {
            bytes: unsafe { std::mem::transmute::<Self, [u8; 4096]>(self) },
        }
    }

    fn is_full(&self) -> bool {
        self.id_count == LEAF_NODE_MAX_LEN as u16
    }

    fn index_to_insert(&self, id: u128) -> usize {
        self.ids
            .iter()
            .position(|&id_| id < id_)
            .unwrap_or(self.id_count as usize)
    }

    /// NOTE: full이 아닐 때만 호출해주세요
    fn insert(&mut self, id: u128) {
        assert!(!self.is_full());

        let index = self.index_to_insert(id);

        // 오른쪽으로 한칸씩 밀고
        if index < self.id_count as usize {
            self.ids
                .copy_within(index..self.id_count as usize, index + 1);
        }
        self.ids[index] = id; // 넣으면
        self.id_count += 1;

        // 정렬된 상태가 유지!
    }

    /// NOTE: full일 때만 호출해주세요
    fn insert_and_split(&mut self, id: u128) -> (LeafNode, u128) {
        assert!(self.is_full());

        let index = self.index_to_insert(id);

        let one_more_ids = {
            let mut ids = [0; LEAF_NODE_MAX_LEN + 1];
            ids[..index].copy_from_slice(&self.ids[..index]);
            ids[index] = id;
            ids[index + 1..].copy_from_slice(&self.ids[index..]);
            ids
        };

        let (left_ids, right_ids) = one_more_ids.split_at(one_more_ids.len() / 2);

        let right_node = LeafNode::new_from_ids(right_ids);

        *self = LeafNode::new_from_ids(left_ids);

        (right_node, right_ids[0])
    }
}

fn main() {
    println!(
        "Size of InternalNode: {}",
        std::mem::size_of::<InternalNode>()
    );
    println!("Size of LeafNode: {}", std::mem::size_of::<LeafNode>());
    println!("Size of Header: {}", std::mem::size_of::<Header>());

    let mut id_set = IdSet::new();

    const N: u128 = 10000;

    for i in 0..N {
        id_set.insert(i);
    }

    for i in 0..N * 2 {
        assert_eq!(id_set.contains(i), i < N, "i: {}", i);
    }

    println!("All tests passed!");
}
