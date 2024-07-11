import Logo from '/logo.svg'

export const Live777Logo = () => {
    return (
        <div class="flex flex-justify-center">
            <a href="https://live777.binbat.com" target="_blank">
                <img
                    src={Logo}
                    class="h-24 mx-2 my-8 transition-[filter] duration-200 ease-in-out hover:drop-shadow-[0_0_1em_#1991e8aa]"
                    alt="Live777 logo"
                />
            </a>
        </div>
    )
}
