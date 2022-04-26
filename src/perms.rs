// SPDX-License-Identifier: Apache-2.0
//! Permissions for a mapping

#![allow(missing_docs)]

macro_rules! perm {
        ($($name:ident[$($trait:ident),* $(,)?] => $value:expr),+ $(,)?) => {
            $(
                #[derive(Debug)]
                pub struct $name;

                impl super::map::Known for $name {
                    const VALUE: libc::c_int = $value;
                }

                $(
                    impl super::map::$trait for $name {}
                )*
            )+
        };
    }

perm! {
    None[] => libc::PROT_NONE,
    Read[Readable] => libc::PROT_READ,
    Write[Writeable] => libc::PROT_WRITE,
    Execute[Executable] => libc::PROT_EXEC,
    ReadWrite[Readable, Writeable] => libc::PROT_READ | libc::PROT_WRITE,
    ReadExecute[Readable, Executable] => libc::PROT_READ | libc::PROT_EXEC,
    WriteExecute[Writeable, Executable] => libc::PROT_WRITE | libc::PROT_EXEC,
    ReadWriteExecute[Readable, Writeable, Executable] => libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
}

pub struct Unknown(pub libc::c_int);

impl super::map::Type for Unknown {
    #[inline]
    fn perms(self) -> libc::c_int {
        self.0
    }
}
