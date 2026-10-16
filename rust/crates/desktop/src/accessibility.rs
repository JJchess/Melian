#![allow(non_snake_case)]
use std::sync::{Arc, Mutex};
use windows::{
    core::{implement, Error, IUnknown, Interface, Result, VARIANT},
    Win32::{
        Foundation::{E_NOINTERFACE, HWND, LPARAM, WPARAM},
        System::{
            Com::SAFEARRAY,
            Ole::{SafeArrayCreateVector, SafeArrayPutElement},
            Variant::VT_I4,
        },
        UI::{Accessibility::*, WindowsAndMessaging::PostMessageW},
    },
};
#[derive(Clone)]
pub struct Item {
    pub id: u32,
    pub name: String,
    pub status: String,
    pub help: String,
    pub rect: UiaRect,
    pub checked: Option<bool>,
}
pub struct Data {
    pub hwnd: usize,
    pub bounds: UiaRect,
    pub items: Vec<Item>,
    pub focused: u32,
    pub status: String,
}
pub type Shared = Arc<Mutex<Data>>;
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    IToggleProvider
)]
pub struct Provider {
    pub data: Shared,
    pub id: u32,
}
impl Provider {
    pub fn simple(data: Shared, id: u32) -> IRawElementProviderSimple {
        Self { data, id }.into()
    }
    fn fragment(data: Shared, id: u32) -> IRawElementProviderFragment {
        Self { data, id }.into()
    }
}
impl IRawElementProviderSimple_Impl for Provider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        if id == UIA_InvokePatternId {
            let p: IInvokeProvider = Provider {
                data: self.data.clone(),
                id: self.id,
            }
            .into();
            p.cast()
        } else if id == UIA_TogglePatternId && self.id == 3 {
            let p: IToggleProvider = Provider {
                data: self.data.clone(),
                id: self.id,
            }
            .into();
            p.cast()
        } else {
            Err(Error::from_hresult(E_NOINTERFACE))
        }
    }
    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let data = self.data.lock().unwrap();
        let item = data.items.iter().find(|i| i.id == self.id);
        Ok(if property == UIA_NamePropertyId {
            let name = if self.id == 0 {
                "Melian".into()
            } else {
                item.map(|i| {
                    if i.status.is_empty() {
                        i.name.clone()
                    } else {
                        format!("{}，{}", i.name, i.status)
                    }
                })
                .unwrap_or_default()
            };
            VARIANT::from(name.as_str())
        } else if property == UIA_ControlTypePropertyId {
            VARIANT::from(if self.id == 3 {
                UIA_CheckBoxControlTypeId.0
            } else if self.id == 0 && !data.items.is_empty() {
                UIA_PaneControlTypeId.0
            } else {
                UIA_ButtonControlTypeId.0
            })
        } else if property == UIA_AutomationIdPropertyId {
            VARIANT::from(format!("ClaudeGuard.{}", self.id).as_str())
        } else if property == UIA_HelpTextPropertyId {
            VARIANT::from(if self.id == 0 {
                data.status.as_str()
            } else {
                item.map(|i| i.help.as_str()).unwrap_or("")
            })
        } else if property == UIA_ItemStatusPropertyId {
            VARIANT::from(item.map(|i| i.status.as_str()).unwrap_or(""))
        } else if property == UIA_IsEnabledPropertyId
            || property == UIA_IsKeyboardFocusablePropertyId
            || property == UIA_IsControlElementPropertyId
            || property == UIA_IsContentElementPropertyId
        {
            VARIANT::from(true)
        } else if property == UIA_HasKeyboardFocusPropertyId {
            VARIANT::from(data.focused == self.id)
        } else {
            VARIANT::default()
        })
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id == 0 {
            unsafe { UiaHostProviderFromHwnd(HWND(self.data.lock().unwrap().hwnd as *mut _)) }
        } else {
            Err(Error::from_hresult(E_NOINTERFACE))
        }
    }
}
impl IRawElementProviderFragment_Impl for Provider_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let data = self.data.lock().unwrap();
        let id = if self.id == 0 {
            if direction == NavigateDirection_FirstChild {
                data.items.first().map(|i| i.id)
            } else if direction == NavigateDirection_LastChild {
                data.items.last().map(|i| i.id)
            } else {
                None
            }
        } else if direction == NavigateDirection_Parent {
            Some(0)
        } else {
            let index = data.items.iter().position(|i| i.id == self.id);
            index
                .and_then(|index| {
                    if direction == NavigateDirection_NextSibling {
                        data.items.get(index + 1)
                    } else if direction == NavigateDirection_PreviousSibling && index > 0 {
                        data.items.get(index - 1)
                    } else {
                        None
                    }
                })
                .map(|i| i.id)
        };
        drop(data);
        id.map(|id| Provider::fragment(self.data.clone(), id))
            .ok_or_else(|| Error::from_hresult(E_NOINTERFACE))
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        unsafe {
            if self.id == 0 {
                return Ok(std::ptr::null_mut());
            }
            let array = SafeArrayCreateVector(VT_I4, 0, 2);
            for (i, v) in [UiaAppendRuntimeId as i32, self.id as i32]
                .iter()
                .enumerate()
            {
                SafeArrayPutElement(array, &(i as i32), (v as *const i32).cast())?;
            }
            Ok(array)
        }
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let data = self.data.lock().unwrap();
        Ok(if self.id == 0 {
            data.bounds
        } else {
            data.items
                .iter()
                .find(|i| i.id == self.id)
                .map(|i| i.rect)
                .unwrap_or_default()
        })
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        unsafe {
            PostMessageW(
                HWND(self.data.lock().unwrap().hwnd as *mut _),
                crate::WM_FOCUS_ITEM,
                WPARAM(self.id as usize),
                LPARAM(0),
            )
        }
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Provider {
            data: self.data.clone(),
            id: 0,
        }
        .into())
    }
}
impl IRawElementProviderFragmentRoot_Impl for Provider_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let data = self.data.lock().unwrap();
        let id = data
            .items
            .iter()
            .find(|i| {
                x >= i.rect.left
                    && x < i.rect.left + i.rect.width
                    && y >= i.rect.top
                    && y < i.rect.top + i.rect.height
            })
            .map(|i| i.id)
            .unwrap_or(0);
        Ok(Provider::fragment(self.data.clone(), id))
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        Ok(Provider::fragment(
            self.data.clone(),
            self.data.lock().unwrap().focused,
        ))
    }
}
impl IInvokeProvider_Impl for Provider_Impl {
    fn Invoke(&self) -> Result<()> {
        unsafe {
            PostMessageW(
                HWND(self.data.lock().unwrap().hwnd as *mut _),
                crate::WM_ACTION,
                WPARAM(self.id as usize),
                LPARAM(0),
            )
        }
    }
}
impl IToggleProvider_Impl for Provider_Impl {
    fn Toggle(&self) -> Result<()> {
        self.Invoke()
    }
    fn ToggleState(&self) -> Result<ToggleState> {
        Ok(
            if self
                .data
                .lock()
                .unwrap()
                .items
                .iter()
                .find(|i| i.id == self.id)
                .and_then(|i| i.checked)
                .unwrap_or(false)
            {
                ToggleState_On
            } else {
                ToggleState_Off
            },
        )
    }
}
