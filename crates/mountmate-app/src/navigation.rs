#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Connections,
    TransferCenter,
    ConnectionEditor,
    Settings,
}

/// Transfers temporarily cover a page; its draft and asynchronous workflow keep running.
pub struct Navigation {
    page: Screen,
    transfers_open: bool,
}

impl Navigation {
    pub fn new(show_transfers: bool) -> Self {
        Self {
            page: Screen::Connections,
            transfers_open: show_transfers,
        }
    }

    pub fn current(&self) -> Screen {
        if self.transfers_open {
            Screen::TransferCenter
        } else {
            self.page
        }
    }

    pub fn context(&self) -> Screen {
        self.page
    }

    pub fn show(&mut self, page: Screen) {
        self.page = page;
    }

    pub fn open_transfers(&mut self) {
        self.transfers_open = true;
    }

    pub fn close_transfers(&mut self) {
        self.transfers_open = false;
    }
}
